// ─── Paso 2: Módulo Provisioning — Portal de configuración WiFi ───
//
// Cuando el dispositivo NO está provisionado, entra en modo Access Point:
// crea una red WiFi propia ("Leonobitech-Setup") donde el usuario se conecta
// y configura las credenciales via un formulario web en http://192.168.4.1
//
// Flujo:
// 1. Device arranca en modo AP (Access Point)
// 2. Usuario se conecta a "Leonobitech-Setup"
// 3. Abre http://192.168.4.1 en el browser
// 4. Llena SSID, password, device ID, API key
// 5. Device guarda credenciales en NVS y reinicia
// 6. Al reiniciar, lee credenciales y conecta como Station

use anyhow::Result;
use embedded_svc::{http::Method, io::Write, ipv4 as embedded_ipv4};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::peripheral,
    http::server::{Configuration as HttpConfig, EspHttpServer},
    netif::{EspNetif, NetifConfiguration, NetifStack},
    wifi::{
        AccessPointConfiguration, AuthMethod, BlockingWifi, Configuration, EspWifi, WifiDriver,
    },
};
use log::{error, info};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use crate::secure_storage::{Credentials, SecureStorage};

// ─── Configuración del Access Point ───

const AP_SSID: &str = "Leonobitech-Setup";
const AP_PASSWORD: &str = "setup1234"; // Mínimo 8 chars para WPA2
const AP_CHANNEL: u8 = 1;
const AP_MAX_CONNECTIONS: u16 = 4;

// ─── HTML del formulario de setup ───

const HTML_FORM: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Leonobitech IoT Setup</title>
    <style>
        body { font-family: -apple-system, sans-serif; max-width: 400px; margin: 50px auto; padding: 20px; background: #1a1a2e; color: #eee; }
        h1 { color: #00d4ff; text-align: center; }
        form { background: #16213e; padding: 20px; border-radius: 10px; }
        label { display: block; margin: 15px 0 5px; color: #00d4ff; }
        input { width: 100%; padding: 12px; border: 1px solid #0f3460; border-radius: 5px; background: #1a1a2e; color: #fff; box-sizing: border-box; }
        button { width: 100%; padding: 15px; margin-top: 20px; background: #00d4ff; color: #1a1a2e; border: none; border-radius: 5px; font-weight: bold; cursor: pointer; }
        button:hover { background: #00a8cc; }
        .info { font-size: 12px; color: #888; margin-top: 5px; }
    </style>
</head>
<body>
    <h1>Leonobitech IoT</h1>
    <form method="POST" action="/provision">
        <label>WiFi Network (SSID)</label>
        <input type="text" name="ssid" required maxlength="32">

        <label>WiFi Password</label>
        <input type="password" name="password" required maxlength="64">

        <label>Device ID</label>
        <input type="text" name="device_id" required maxlength="32">
        <div class="info">Unique identifier for this device</div>

        <label>API Key</label>
        <input type="password" name="api_key" maxlength="128">
        <div class="info">Optional: For authenticated API calls</div>

        <button type="submit">Save & Connect</button>
    </form>
</body>
</html>"#;

const HTML_SUCCESS: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Setup Complete</title>
    <style>
        body { font-family: -apple-system, sans-serif; max-width: 400px; margin: 50px auto; padding: 20px; background: #1a1a2e; color: #eee; text-align: center; }
        h1 { color: #00ff88; }
        p { color: #888; }
    </style>
</head>
<body>
    <h1>Setup Complete!</h1>
    <p>Device will restart and connect to your WiFi network.</p>
    <p>This access point will disappear.</p>
</body>
</html>"#;

// ─── Función principal de provisioning ───

/// Inicia el modo provisioning: crea un Access Point con servidor HTTP.
///
/// Esta función NUNCA retorna normalmente — al completar el provisioning,
/// reinicia el chip con esp_restart().
pub fn start_provisioning(
    modem: impl peripheral::Peripheral<P = esp_idf_svc::hal::modem::Modem> + 'static,
    sysloop: EspSystemEventLoop,
    storage: Arc<Mutex<SecureStorage>>,
) -> Result<()> {
    info!("Starting provisioning mode...");

    // ─── Configurar WiFi en modo Access Point ───

    // WifiDriver es el driver de bajo nivel (más control que EspWifi directo)
    let driver = WifiDriver::new(modem, sysloop.clone(), None)?;

    // Necesitamos dos interfaces de red:
    // - STA (Station): necesaria internamente aunque no la usemos
    // - AP (Access Point): la que crea nuestra red WiFi
    let sta_netif = EspNetif::new(NetifStack::Sta)?;

    // Configurar AP con IP estática 192.168.4.1 y DHCP server
    let ap_netif_config = NetifConfiguration {
        flags: 0,
        got_ip_event_id: None,
        lost_ip_event_id: None,
        key: "WIFI_AP_DEF".try_into().unwrap(),
        description: "ap".try_into().unwrap(),
        route_priority: 10,
        ip_configuration: Some(embedded_ipv4::Configuration::Router(
            embedded_ipv4::RouterConfiguration {
                subnet: embedded_ipv4::Subnet {
                    gateway: Ipv4Addr::new(192, 168, 4, 1),
                    mask: embedded_ipv4::Mask(24),
                },
                dhcp_enabled: true,
                dns: None,
                secondary_dns: None,
            },
        )),
        stack: NetifStack::Ap,
        custom_mac: None,
    };

    let ap_netif = EspNetif::new_with_conf(&ap_netif_config)?;

    // wrap_all() combina driver + ambas interfaces en un EspWifi
    let mut wifi = EspWifi::wrap_all(driver, sta_netif, ap_netif)?;
    let mut blocking_wifi = BlockingWifi::wrap(&mut wifi, sysloop)?;

    // Configurar el Access Point
    let ap_config = AccessPointConfiguration {
        ssid: AP_SSID.try_into().unwrap(),
        password: AP_PASSWORD.try_into().unwrap(),
        channel: AP_CHANNEL,
        auth_method: AuthMethod::WPA2Personal,
        max_connections: AP_MAX_CONNECTIONS,
        ..Default::default()
    };

    blocking_wifi.set_configuration(&Configuration::AccessPoint(ap_config))?;
    blocking_wifi.start()?;

    // Esperar a que la interfaz de red esté lista
    std::thread::sleep(std::time::Duration::from_millis(500));

    info!("Provisioning mode active");

    // ─── Flag de provisioning completado ───

    // Arc<Mutex<bool>> para compartir estado entre el handler HTTP y el loop principal.
    // El handler HTTP corre en otro thread (del servidor), necesita acceso compartido.
    let provisioned = Arc::new(Mutex::new(false));
    let provisioned_clone = provisioned.clone();
    let storage_clone = storage.clone();

    // ─── Servidor HTTP ───

    let mut server = EspHttpServer::new(&HttpConfig::default())?;

    // GET / → Servir formulario HTML
    server.fn_handler(
        "/",
        Method::Get,
        |req| -> core::result::Result<(), esp_idf_svc::io::EspIOError> {
            let mut response = req.into_ok_response()?;
            response.write_all(HTML_FORM.as_bytes())?;
            Ok(())
        },
    )?;

    // POST /provision → Procesar formulario
    // `move` transfiere ownership de provisioned_clone y storage_clone al closure.
    // Sin `move`, el closure intentaría tomar referencias — pero el closure
    // vive más que la función actual, así que necesita ownership.
    server.fn_handler(
        "/provision",
        Method::Post,
        move |mut req| -> core::result::Result<(), esp_idf_svc::io::EspIOError> {
            // Leer body del POST (formulario URL-encoded)
            let mut body = [0u8; 512];
            let len = req.read(&mut body)?;
            let body_str = std::str::from_utf8(&body[..len]).unwrap_or("");

            // Parsear campos del formulario
            let mut ssid = String::new();
            let mut password = String::new();
            let mut device_id = String::new();
            let mut api_key = String::new();

            for pair in body_str.split('&') {
                if let Some((key, value)) = pair.split_once('=') {
                    let decoded = urlencoding_decode(value);
                    match key {
                        "ssid" => ssid = decoded,
                        "password" => password = decoded,
                        "device_id" => device_id = decoded,
                        "api_key" => api_key = decoded,
                        _ => {}
                    }
                }
            }

            // Validar campos requeridos
            if ssid.is_empty() || password.is_empty() || device_id.is_empty() {
                error!("Missing required fields in provisioning form");
                let mut response = req.into_status_response(400)?;
                response.write_all(b"Missing required fields")?;
                return Ok(());
            }

            // Crear credenciales y guardar en NVS
            let creds = Credentials {
                wifi_ssid: ssid,
                wifi_password: password,
                api_key,
                device_id,
            };

            if let Ok(mut storage) = storage_clone.lock() {
                if let Err(e) = storage.store_credentials(creds) {
                    error!("Failed to store credentials: {:?}", e);
                    let mut response = req.into_status_response(500)?;
                    response.write_all(b"Failed to store credentials")?;
                    return Ok(());
                }
            }

            // Marcar provisioning como completado
            if let Ok(mut p) = provisioned_clone.lock() {
                *p = true;
            }

            // Enviar página de éxito
            let mut response = req.into_ok_response()?;
            response.write_all(HTML_SUCCESS.as_bytes())?;

            info!("Provisioning complete! Device will restart.");
            Ok(())
        },
    )?;

    // ─── Loop de espera ───

    // Esperar hasta que el usuario complete el formulario.
    // El handler HTTP setea provisioned = true, y este loop lo detecta.
    info!("Waiting for user to complete setup...");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));

        if let Ok(p) = provisioned.lock() {
            if *p {
                info!("Provisioning completed, restarting in 3 seconds...");
                std::thread::sleep(std::time::Duration::from_secs(3));
                unsafe {
                    esp_idf_svc::sys::esp_restart();
                }
            }
        }
    }
}

// ─── URL Decoding ───

/// Decodifica URL encoding simple (maneja %XX y + como espacio).
///
/// Los formularios HTML envían datos como "ssid=Mi+Red&password=abc%21".
/// Esta función convierte eso de vuelta a texto legible.
fn urlencoding_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '+' => result.push(' '),
            '%' => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                }
            }
            _ => result.push(c),
        }
    }

    result
}
