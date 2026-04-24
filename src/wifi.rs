// ─── Paso 2: Módulo WiFi — Conexión a red WiFi ───
//
// Este módulo encapsula toda la lógica de conexión WiFi en una sola
// función pública: connect(). Recibe credenciales y el periférico modem,
// y retorna el driver WiFi conectado con IP asignada.

use anyhow::{bail, Result};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::peripheral,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use log::info;

/// Conecta el ESP32 a una red WiFi en modo Station.
///
/// Retorna `Box<EspWifi<'static>>` — el driver WiFi en el heap.
/// IMPORTANTE: mientras el Box exista, la conexión WiFi se mantiene.
/// Si se dropea, la conexión se pierde (RAII).
pub fn connect(
    ssid: &str,
    password: &str,
    modem: impl peripheral::Peripheral<P = esp_idf_svc::hal::modem::Modem> + 'static,
    sysloop: EspSystemEventLoop,
) -> Result<Box<EspWifi<'static>>> {
    // ─── Validación de credenciales ───

    let mut auth_method = AuthMethod::WPA2Personal;
    if ssid.is_empty() {
        bail!("WiFi SSID not configured");
    }

    // Debug: loguear longitud del password (nunca el password en sí)
    info!("WiFi password length: {} bytes", password.len());

    if password.is_empty() {
        auth_method = AuthMethod::None;
        info!("WiFi password empty, using open network");
    }

    // ─── Crear driver WiFi ───

    // EspWifi::new() toma ownership del modem — nadie más puede usar el radio.
    // sysloop.clone() es barato: usa Arc internamente (solo incrementa contador).
    // None = sin NVS partition (no persistimos config WiFi en flash).
    let mut esp_wifi = EspWifi::new(modem, sysloop.clone(), None)?;

    // BlockingWifi wrappea el driver async en API síncrona.
    // Usa &mut (borrow) — NO toma ownership de esp_wifi.
    let mut wifi = BlockingWifi::wrap(&mut esp_wifi, sysloop)?;

    // Configuración default para poder hacer start() y scan()
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;

    info!("Starting WiFi...");
    wifi.start()?;

    // ─── Scan de redes ───

    // Escaneamos para encontrar el canal exacto del AP.
    // Con el canal correcto, la conexión es más rápida.
    info!("Scanning for networks...");
    let ap_infos = wifi.scan()?;

    let target_ap = ap_infos.into_iter().find(|ap| ap.ssid == ssid);

    let channel = target_ap.as_ref().map(|ap| ap.channel);

    info!("Found AP '{}' on channel {:?}", ssid, channel.unwrap_or(0));

    // ─── Configurar con credenciales reales ───

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().expect("SSID too long"),
        password: password.try_into().expect("Password too long"),
        channel,
        auth_method,
        ..Default::default()
    }))?;

    // ─── Conectar y obtener IP ───

    // connect() bloquea hasta autenticación con el router
    info!("Connecting to '{}'...", ssid);
    wifi.connect()?;

    // wait_netif_up() bloquea hasta obtener IP por DHCP
    // Sin IP no podemos hacer nada en la red (ni HTTP, ni DNS)
    info!("Waiting for DHCP lease...");
    wifi.wait_netif_up()?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;

    info!("WiFi connected!");
    info!("IP: {}", ip_info.ip);
    info!("Gateway: {}", ip_info.subnet.gateway);
    info!("Mask: {}", ip_info.subnet.mask);

    // Retornamos esp_wifi en un Box (heap allocation).
    // BlockingWifi se dropea aquí, pero la conexión sigue porque
    // el driver real (esp_wifi) sigue vivo en el Box.
    Ok(Box::new(esp_wifi))
}
