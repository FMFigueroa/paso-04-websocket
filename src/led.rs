// ─── Paso 3: Módulo LED — Control del WS2812 on-board vía RMT ───
//
// El LED binario del paso 2 (on/off con GPIO) deja de alcanzar cuando
// querés matiz. La solución que adoptamos es modular el WS2812 RGB
// on-board del DevKit vía el periférico RMT (Remote Control Transceiver).
//
// RMT genera pulsos de timing preciso (nanosegundos) por hardware — sin
// consumir CPU. El WS2812 recibe esos pulsos y los interpreta como datos
// de color: 24 bits por LED (8 G + 8 R + 8 B).
//
// Para simular "intensidad" con un LED RGB usamos GRADIENTE DE GRIS:
// R = G = B = component, donde component es proporcional al percent.
// A percent=50, el LED brilla en "medio blanco" (RGB=128,128,128).
//
// Hardware: ninguno. El WS2812 está cableado a GPIO8 en el DevKit.

use anyhow::Result;
use esp_idf_hal::gpio::OutputPin;
use esp_idf_hal::peripheral::Peripheral;
use esp_idf_hal::rmt::RmtChannel;
use log::info;
use smart_leds_trait::{SmartLedsWrite, RGB8};
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

// ─── Estructura del controller ───

/// Controller del LED RGB on-board por gradiente de gris.
///
/// Envuelve el driver `Ws2812Esp32Rmt` del crate `ws2812-esp32-rmt-driver`
/// y expone una API simple: `set_brightness(0..100)`. Internamente mapea
/// el porcentaje a una componente u8 (0..255) y emite un pixel RGB
/// donde R = G = B = component — un blanco con la intensidad pedida.
pub struct LedController<'d> {
    driver: Ws2812Esp32Rmt<'d>,
}

impl<'d> LedController<'d> {
    /// Inicializa el controller sobre un channel RMT y un pin GPIO.
    ///
    /// Ambos recursos se mueven al `Ws2812Esp32Rmt` interno y quedan
    /// "casados" con este controller. Si se dropea, el channel RMT se
    /// libera y el pin vuelve a alta impedancia (RAII).
    pub fn new(
        channel: impl Peripheral<P = impl RmtChannel> + 'd,
        pin: impl Peripheral<P = impl OutputPin> + 'd,
    ) -> Result<Self> {
        let driver = Ws2812Esp32Rmt::new(channel, pin)?;
        info!("WS2812 LED initialized on RMT channel");
        Ok(Self { driver })
    }

    /// Ajusta el brillo del LED como un gris uniforme.
    ///
    /// `percent` ∈ [0, 100]. Valores mayores se clampean (saturación).
    /// 0 = apagado (RGB 0,0,0); 100 = brillo máximo (RGB 255,255,255).
    ///
    /// La API externa es IDÉNTICA a la que tenía la primera versión de
    /// paso-03 (misma API, diferente driver por detrás) — eso es intencional. Los pasos
    /// 04-09 llaman `set_brightness(u8)` sin saber si por detrás hay un
    /// driver PWM o un driver RMT/WS2812.
    pub fn set_brightness(&mut self, percent: u8) -> Result<()> {
        let percent = percent.min(100) as u32;

        // Regla de tres entera: component = percent * 255 / 100.
        // Multiplicar ANTES de dividir para no perder precisión en
        // aritmética entera (mismo patrón que vimos en la versión PWM).
        let component = ((percent * 255) / 100) as u8;

        let pixels = [RGB8::new(component, component, component)];
        self.driver.write(pixels.iter().cloned())?;
        Ok(())
    }

    /// Apaga el LED (RGB 0,0,0).
    ///
    /// Equivalente a `set_brightness(0)`, pero nombrado explícito para
    /// que el intent del caller quede claro en los call sites.
    #[allow(dead_code)]
    pub fn off(&mut self) -> Result<()> {
        let pixels = [RGB8::new(0, 0, 0)];
        self.driver.write(pixels.iter().cloned())?;
        Ok(())
    }
}
