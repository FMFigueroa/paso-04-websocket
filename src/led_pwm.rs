// ─── Paso 3: Módulo LED PWM — Control de brillo por modulación ───
//
// El LED binario del paso 2 (on/off con GPIO) deja de alcanzar cuando
// querés brillo intermedio. La solución estándar es PWM (Pulse Width
// Modulation): prendés y apagás el pin muchas veces por segundo, y el
// ojo humano integra los pulsos como un brillo continuo.
//
// Este módulo encapsula el periférico LEDC del ESP32-C3 (LED PWM
// Controller) detrás de una API simple: set_brightness(0..100).
//
// Hardware: requiere un LED externo en GPIO10 con resistor de 220Ω
// (el GPIO8 está cableado al WS2812 on-board, que NO responde a PWM
// clásico — interpreta los pulsos como datos de color).

use anyhow::Result;
use esp_idf_hal::gpio::OutputPin;
use esp_idf_hal::ledc::{
    config::TimerConfig, LedcChannel, LedcDriver, LedcTimer, LedcTimerDriver, Resolution,
};
use esp_idf_hal::peripheral::Peripheral;
use esp_idf_hal::units::Hertz;
use log::info;

// ─── Configuración del PWM ───

/// Frecuencia de la señal PWM.
///
/// 5 kHz es un punto dulce: lo suficientemente rápido para que el ojo
/// humano no perciba flicker (umbral ≈ 100 Hz), pero no tan alto como
/// para desperdiciar ciclos del timer.
const PWM_FREQUENCY_HZ: u32 = 5_000;

/// Resolución del duty cycle.
///
/// 13 bits = 2^13 = 8192 niveles distintos de brillo. Más granularidad
/// que suficiente para el ojo humano (distingue ~100 niveles en el
/// rango útil). A 5 kHz, 13 bits es el sweet spot del LEDC del C3.
const PWM_RESOLUTION: Resolution = Resolution::Bits13;

// ─── Estructura del controller ───

/// Controller del LED por PWM.
///
/// Encapsula el driver LEDC + el max_duty calculado a partir de la
/// resolución. Expone `set_brightness(percent)` donde percent ∈ [0, 100].
/// La conversión a duty cycle se hace internamente.
pub struct LedController<'d> {
    driver: LedcDriver<'d>,
    max_duty: u32,
}

impl<'d> LedController<'d> {
    /// Inicializa el controller sobre un timer, channel y pin.
    ///
    /// Los tres recursos vienen del `Peripherals::take()` del main —
    /// se mueven acá y quedan "casados" con este controller para
    /// siempre. Si se dropea el controller, los tres se liberan (RAII).
    pub fn new(
        timer: impl Peripheral<P = impl LedcTimer> + 'd,
        channel: impl Peripheral<P = impl LedcChannel> + 'd,
        pin: impl Peripheral<P = impl OutputPin> + 'd,
    ) -> Result<Self> {
        // El TimerDriver configura la frecuencia y resolución del timer.
        // Todos los channels asociados a este timer comparten esos params.
        let timer_config = TimerConfig::new()
            .frequency(Hertz(PWM_FREQUENCY_HZ))
            .resolution(PWM_RESOLUTION);

        let timer_driver = LedcTimerDriver::new(timer, &timer_config)?;

        // El LedcDriver asocia un channel del hardware con un timer y un pin.
        // Una vez construido, exponés .set_duty() para cambiar el brillo.
        let driver = LedcDriver::new(channel, timer_driver, pin)?;

        // max_duty depende de la resolución: 13 bits → 2^13 - 1 = 8191.
        // Lo leemos del driver en lugar de hardcodearlo para que el mapping
        // percent→duty siga funcionando si cambiamos la resolución.
        let max_duty = driver.get_max_duty();

        info!(
            "LED PWM initialized: freq={} Hz, resolution=13-bit, max_duty={}",
            PWM_FREQUENCY_HZ, max_duty
        );

        Ok(Self { driver, max_duty })
    }

    /// Ajusta el brillo del LED.
    ///
    /// `percent` ∈ [0, 100]. Valores mayores a 100 se clampean (saturación).
    /// 0 = apagado, 100 = brillo máximo.
    pub fn set_brightness(&mut self, percent: u8) -> Result<()> {
        // .min(100) previene overflow si alguien pasa 255 por accidente.
        let percent = percent.min(100) as u32;

        // Regla de tres entera: duty = max_duty * percent / 100.
        // Multiplicar ANTES de dividir para no perder precisión.
        let duty = (self.max_duty * percent) / 100;

        self.driver.set_duty(duty)?;
        Ok(())
    }

    /// Apaga el LED (duty = 0).
    ///
    /// Equivalente a `set_brightness(0)`, pero nombrado explícito para
    /// que el intent del caller quede claro en los call sites.
    #[allow(dead_code)]
    pub fn off(&mut self) -> Result<()> {
        self.driver.set_duty(0)?;
        Ok(())
    }
}
