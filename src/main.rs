// ─── Paso 4: WebSocket Client — El device habla con un backend ───
//
// El LED de paso-03 respira solo. Acá agregamos una conexión WSS
// persistente al backend de eco (`ws.postman-echo.com`) y le damos al
// usuario la posibilidad de mandar comandos remotos tipo
// `{"type":"SetBrightness","percent":75}`. Cuando llega uno, el LED
// queda fijo en ese brillo por 3 s y después vuelve a la respiración.
//
// Módulos nuevos: ws_client
// Módulos heredados intactos: wifi, secure_storage, provisioning, led

// ─── Módulos ───

mod led;
mod provisioning;
mod secure_storage;
mod wifi;
mod ws_client;

// ─── Imports ───

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

#[allow(unused_imports)]
use esp_idf_svc::sys as _;

use log::{error, info, warn};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use led::LedController;
use secure_storage::SecureStorage;
use ws_client::{OutgoingMessage, WsClient};

// ─── Configuración del firmware ───

/// Secuencia de brillo para el ciclo de respiración. Misma que paso-03.
const BRIGHTNESS_STEPS: &[u8] = &[0, 25, 50, 75, 100, 75, 50, 25];
const BREATH_STEP_MS: u32 = 500;

// ─── Punto de entrada ───

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("paso-04-websocket");

    if let Err(e) = run() {
        error!("Error fatal: {:?}", e);
        error!("Reiniciando en 10 segundos...");
        std::thread::sleep(Duration::from_secs(10));
        unsafe {
            esp_idf_svc::sys::esp_restart();
        }
    }
}

fn run() -> anyhow::Result<()> {
    // ─── Inicialización del sistema ───

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs_partition = EspDefaultNvsPartition::take()?;

    // ─── LED con PWM (heredado de paso-03) ───

    let led_controller = LedController::new(peripherals.rmt.channel0, peripherals.pins.gpio2)?;
    let led = Arc::new(Mutex::new(led_controller));

    info!("LED WS2812 configurado en GPIO2 (RMT channel 0)");

    // ─── Secure Storage (heredado de paso-02) ───

    let storage = SecureStorage::new(nvs_partition.clone())?;
    let storage = Arc::new(Mutex::new(storage));

    // ─── Check de provisioning ───

    let is_provisioned = {
        let s = storage.lock().unwrap();
        s.is_provisioned()?
    };

    if !is_provisioned {
        warn!("Device not provisioned!");
        info!("Starting provisioning mode...");
        info!("Connect to WiFi: 'Leonobitech-Setup' / Password: 'setup1234'");
        info!("Then open http://192.168.4.1 in your browser");
        provisioning::start_provisioning(peripherals.modem, sysloop, storage)?;
        return Ok(());
    }

    // ─── Conectar a WiFi ───

    info!("Device is provisioned, loading credentials...");

    let credentials = {
        let s = storage.lock().unwrap();
        s.load_credentials()?
    };

    let device_id = credentials.device_id.clone();

    info!("Device ID: {}", device_id);
    info!("Connecting to WiFi: {}", credentials.wifi_ssid);

    let _wifi = wifi::connect(
        &credentials.wifi_ssid,
        &credentials.wifi_password,
        peripherals.modem,
        sysloop,
    )?;

    info!("WiFi connected!");

    // Zeroizar credenciales ANTES de abrir el WS — los secretos ya no
    // nos hacen falta en memoria. Solo retenemos device_id (público).
    drop(credentials);
    info!("Credentials zeroized from memory");

    // ─── WebSocket client + override del LED ───
    //
    // `override_until` guarda el Instant hasta el que el LED está
    // "manual" (WS dictó un brillo). Mientras Some(t) y t > now(), el
    // loop de respiración NO toca el LED. Cuando t pasa, el loop
    // retoma la respiración desde el siguiente paso.

    let override_until: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    let ws = WsClient::new(led.clone(), override_until.clone())?;

    // Saludamos al backend. El echo server devuelve el mismo mensaje,
    // así que esperamos ver un `WS ↩ Hello echoed` en los logs.
    ws.send(OutgoingMessage::Hello {
        device_id: device_id.clone(),
    })?;

    // ─── Loop principal: respiración + override ───
    //
    // El loop tick cada 500 ms:
    //   - Si override_until está en el futuro → no tocar el LED
    //   - Si no → aplicar BRIGHTNESS_STEPS[step_idx] y avanzar
    //
    // Esto implementa el "modelo de prioridades" más simple posible.
    // Paso-05 lo va a generalizar con LightState { mode, brightness, ... }.

    info!("Entering main loop — LED breathing with WS override enabled");
    info!(
        "Send a command with: websocat wss://ws.postman-echo.com/raw (then paste a JSON payload)"
    );

    let mut step_idx: usize = 0;
    loop {
        let now = Instant::now();

        // Check no bloqueante: ¿estamos en override?
        let in_override = {
            let guard = override_until.lock().unwrap();
            matches!(*guard, Some(t) if t > now)
        };

        if in_override {
            // El WS dictó un brillo — lo respetamos por los 3 s del override.
            // No tocamos el LED ni avanzamos step_idx (así la respiración
            // retoma suavemente desde donde iba).
        } else {
            let step = BRIGHTNESS_STEPS[step_idx];
            {
                let mut l = led.lock().unwrap();
                l.set_brightness(step)?;
            }
            info!("Breathing: {}%", step);
            step_idx = (step_idx + 1) % BRIGHTNESS_STEPS.len();
        }

        FreeRtos::delay_ms(BREATH_STEP_MS);
    }
}
