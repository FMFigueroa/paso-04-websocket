// ─── Paso 2: Módulo Secure Storage — Credenciales en NVS ───
//
// NVS (Non-Volatile Storage) es la "flash persistente" del ESP32.
// Sobrevive reinicios y power cycles. Es como un key-value store
// guardado en una partición dedicada de la flash.
//
// Las credenciales se borran de memoria automáticamente al salir
// de scope gracias a Zeroize/ZeroizeOnDrop.

use anyhow::{bail, Result};
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use log::{info, warn};
use zeroize::{Zeroize, ZeroizeOnDrop};

// ─── Constantes NVS ───

const NVS_NAMESPACE: &str = "credentials";
const KEY_WIFI_SSID: &str = "wifi_ssid";
const KEY_WIFI_PASS: &str = "wifi_pass";
const KEY_API_KEY: &str = "api_key";
const KEY_DEVICE_ID: &str = "device_id";
const KEY_PROVISIONED: &str = "provisioned";

// ─── Struct de credenciales con borrado seguro ───

/// Contenedor de credenciales que se borra automáticamente de memoria.
///
/// `Zeroize` permite llamar .zeroize() manualmente.
/// `ZeroizeOnDrop` lo hace automáticamente cuando el struct sale de scope.
/// Esto previene que credenciales queden en memoria después de usarlas.
#[derive(Debug, Default, Zeroize, ZeroizeOnDrop)]
pub struct Credentials {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub api_key: String,
    pub device_id: String,
}

// ─── Secure Storage Manager ───

/// Manager de almacenamiento seguro usando NVS del ESP32.
///
/// Encapsula un handle de NVS con namespace "credentials".
/// Todas las operaciones de lectura/escritura pasan por aquí.
pub struct SecureStorage {
    nvs: EspNvs<NvsDefault>,
}

impl SecureStorage {
    /// Inicializa el storage con la partición NVS default.
    /// `true` en EspNvs::new = crear namespace si no existe.
    pub fn new(nvs_partition: EspNvsPartition<NvsDefault>) -> Result<Self> {
        let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
        info!(
            "SecureStorage initialized with namespace: {}",
            NVS_NAMESPACE
        );
        Ok(Self { nvs })
    }

    /// Verifica si el dispositivo ya fue provisionado.
    /// Lee un flag u8 de NVS: 1 = provisionado, 0 o ausente = no.
    pub fn is_provisioned(&self) -> Result<bool> {
        match self.nvs.get_u8(KEY_PROVISIONED) {
            Ok(Some(val)) => Ok(val == 1),
            Ok(None) => Ok(false),
            Err(e) => {
                warn!("Error checking provisioned status: {:?}", e);
                Ok(false)
            }
        }
    }

    /// Guarda credenciales en NVS y las zeroiza de la entrada.
    ///
    /// Cada campo se guarda como string independiente en NVS.
    /// Al final, marca el flag "provisioned" = 1.
    pub fn store_credentials(&mut self, mut creds: Credentials) -> Result<()> {
        self.nvs.set_str(KEY_WIFI_SSID, &creds.wifi_ssid)?;
        self.nvs.set_str(KEY_WIFI_PASS, &creds.wifi_password)?;
        self.nvs.set_str(KEY_API_KEY, &creds.api_key)?;
        self.nvs.set_str(KEY_DEVICE_ID, &creds.device_id)?;

        // Marcar como provisionado
        self.nvs.set_u8(KEY_PROVISIONED, 1)?;

        // Zeroizar explícitamente las credenciales de entrada
        creds.zeroize();

        info!("Credentials stored securely");
        Ok(())
    }

    /// Carga credenciales desde NVS.
    ///
    /// Retorna un Credentials con ZeroizeOnDrop — al salir de scope
    /// se borra automáticamente de memoria.
    pub fn load_credentials(&self) -> Result<Credentials> {
        if !self.is_provisioned()? {
            bail!("Device not provisioned. Run provisioning first.");
        }

        let mut creds = Credentials::default();

        // Buffer temporal para lecturas — también se zeroiza después de cada uso
        let mut buf = [0u8; 256];

        if let Some(val) = self.nvs.get_str(KEY_WIFI_SSID, &mut buf)? {
            creds.wifi_ssid = val.trim_end_matches('\0').to_string();
            buf.zeroize();
        }

        if let Some(val) = self.nvs.get_str(KEY_WIFI_PASS, &mut buf)? {
            creds.wifi_password = val.trim_end_matches('\0').to_string();
            buf.zeroize();
        }

        if let Some(val) = self.nvs.get_str(KEY_API_KEY, &mut buf)? {
            creds.api_key = val.trim_end_matches('\0').to_string();
            buf.zeroize();
        }

        if let Some(val) = self.nvs.get_str(KEY_DEVICE_ID, &mut buf)? {
            creds.device_id = val.trim_end_matches('\0').to_string();
            buf.zeroize();
        }

        info!("Credentials loaded from NVS");
        Ok(creds)
    }

    /// Borra todas las credenciales de NVS (factory reset).
    ///
    /// Sobreescribe con strings vacíos antes de marcar como no provisionado.
    #[allow(dead_code)]
    pub fn clear_credentials(&mut self) -> Result<()> {
        warn!("Clearing all credentials from NVS...");

        self.nvs.set_str(KEY_WIFI_SSID, "")?;
        self.nvs.set_str(KEY_WIFI_PASS, "")?;
        self.nvs.set_str(KEY_API_KEY, "")?;
        self.nvs.set_str(KEY_DEVICE_ID, "")?;
        self.nvs.set_u8(KEY_PROVISIONED, 0)?;

        info!("Credentials cleared");
        Ok(())
    }
}
