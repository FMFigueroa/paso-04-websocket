// ─── Paso 4: Módulo WebSocket Client — FFI directo a esp_transport_ws ───
//
// El ESP-IDF provee una API de WebSocket de alto nivel (`esp_idf_svc::ws::
// client::EspWebSocketClient`) pero requiere agregar un componente extra
// al SDK (`esp-websocket-client`). Para mantener el build limpio, vamos
// directo al TRANSPORT layer: `esp_transport_ws_*` + `esp_transport_ssl_*`.
//
// Es más verboso — manejamos opcodes WS a mano, frames byte-level, y
// handshake TLS explícito. Pero compila out-of-the-box y es fiel a lo que
// hace el firmware productivo (ver `hardware/src/ws_client.rs` del
// proyecto madre Leonobitech).
//
// Stack del módulo:
// - `esp_transport_ssl_*` para el handshake TLS + CA bundle
// - `esp_transport_ws_*` para la lógica del protocolo WebSocket
// - Opcodes manuales (TEXT, PING, PONG, CLOSE) con el flag FIN
// - serde + serde_json para (de)serializar enums tipados
// - std::thread + mpsc::channel para comunicar main ↔ WS thread

use anyhow::{anyhow, Result};
use esp_idf_svc::sys::*;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::ptr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::led::LedController;

// ─── Configuración de la conexión ───

const WS_HOST: &str = "ws.postman-echo.com";
const WS_PORT: i32 = 443;
const WS_PATH: &str = "/raw";

const CONNECT_TIMEOUT_MS: i32 = 10_000;
const READ_TIMEOUT_MS: i32 = 100;
const RECONNECT_DELAY_SECS: u64 = 5;

/// Cuánto dura un override del LED antes de volver a la respiración.
const OVERRIDE_DURATION_SECS: u64 = 3;

/// Stack del thread del WS. El handshake TLS + parseo JSON consume más
/// que el default de FreeRTOS. 16 KB es el mismo valor que usa el horizonte.
const WS_THREAD_STACK_BYTES: usize = 16 * 1024;

// ─── Opcodes del protocolo WebSocket (RFC 6455 §5.2) ───
//
// Cada frame WS empieza con un byte donde los 4 bits bajos son el opcode
// y el bit alto (0x80) es FIN — "este es el último fragmento del mensaje".
// Un mensaje de texto corto se manda con un solo frame: opcode=TEXT_FIN.

const WS_OPCODE_FIN: u8 = 0x80;
const WS_OPCODE_TEXT: u8 = 0x01;
const WS_OPCODE_CLOSE: u8 = 0x08;
const WS_OPCODE_PING: u8 = 0x09;
const WS_OPCODE_PONG: u8 = 0x0A;
const WS_OPCODE_TEXT_FIN: u8 = WS_OPCODE_TEXT | WS_OPCODE_FIN; // 0x81

// ─── Schema de mensajes (paso-04: mínimo funcional) ───
//
// Pedagógicamente simple: un Hello al conectar, un Echo opcional, y un
// SetBrightness que el user puede mandar desde terminal con `websocat`.
// Los pasos 05+ van a extender este enum.

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    Hello {
        device_id: String,
    },
    #[allow(dead_code)]
    Echo {
        text: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    Hello { device_id: String },
    Echo { text: String },
    SetBrightness { percent: u8 },
}

// ─── Struct público ───

pub struct WsClient {
    outbound_tx: Sender<OutgoingMessage>,
}

impl WsClient {
    /// Crea un cliente nuevo y spawnea el thread de fondo.
    pub fn new(
        led: Arc<Mutex<LedController<'static>>>,
        override_until: Arc<Mutex<Option<Instant>>>,
    ) -> Result<Self> {
        let (outbound_tx, outbound_rx) = mpsc::channel::<OutgoingMessage>();

        thread::Builder::new()
            .name("ws_client".into())
            .stack_size(WS_THREAD_STACK_BYTES)
            .spawn(move || {
                ws_connection_loop(outbound_rx, led, override_until);
            })?;

        info!(
            "WS client thread spawned (stack={} bytes)",
            WS_THREAD_STACK_BYTES
        );
        Ok(Self { outbound_tx })
    }

    /// Encola un mensaje outbound. No bloquea.
    pub fn send(&self, msg: OutgoingMessage) -> Result<()> {
        self.outbound_tx
            .send(msg)
            .map_err(|e| anyhow!("ws send failed: {}", e))
    }
}

// ─── Loop de conexión / reconexión ───

fn ws_connection_loop(
    outbound_rx: Receiver<OutgoingMessage>,
    led: Arc<Mutex<LedController<'static>>>,
    override_until: Arc<Mutex<Option<Instant>>>,
) {
    loop {
        info!("WS: connecting to wss://{}{}", WS_HOST, WS_PATH);

        match connect_and_run(&outbound_rx, &led, &override_until) {
            Ok(_) => info!("WS: disconnected normally"),
            Err(e) => error!("WS error: {:?}", e),
        }

        info!("WS: reconnecting in {}s...", RECONNECT_DELAY_SECS);
        thread::sleep(Duration::from_secs(RECONNECT_DELAY_SECS));
    }
}

/// Conexión + loop de I/O.
///
/// Retorna cuando la conexión cae (error o CLOSE del servidor).
/// El caller la vuelve a llamar después del delay de reconexión.
fn connect_and_run(
    outbound_rx: &Receiver<OutgoingMessage>,
    led: &Arc<Mutex<LedController<'static>>>,
    override_until: &Arc<Mutex<Option<Instant>>>,
) -> Result<()> {
    // ─── 1. SSL transport ───
    //
    // esp_transport_ssl_init() crea un handle para una conexión TLS.
    // Después habilitamos el CA bundle global (para verificar el cert
    // del server) y le atachamos la función del SDK que provee el bundle.

    let ssl = unsafe { esp_transport_ssl_init() };
    if ssl.is_null() {
        return Err(anyhow!("Failed to create SSL transport"));
    }

    unsafe {
        esp_transport_ssl_enable_global_ca_store(ssl);
        esp_transport_ssl_crt_bundle_attach(ssl, Some(esp_crt_bundle_attach));
    }

    // ─── 2. WS transport sobre el SSL ───
    //
    // esp_transport_ws_init() wrappea el SSL transport con la lógica WS:
    // handshake HTTP Upgrade, framing, masking client-side (RFC 6455).

    let ws = unsafe { esp_transport_ws_init(ssl) };
    if ws.is_null() {
        unsafe {
            esp_transport_destroy(ssl);
        }
        return Err(anyhow!("Failed to create WS transport"));
    }

    // Path del endpoint. Los bytes NO pueden contener NUL — CString garantiza.
    let path = CString::new(WS_PATH).map_err(|e| anyhow!("bad path: {}", e))?;
    unsafe {
        esp_transport_ws_set_path(ws, path.as_ptr());
    }

    // ─── 3. Conectar (handshake TLS + HTTP Upgrade en una sola call) ───

    let host = CString::new(WS_HOST).map_err(|e| anyhow!("bad host: {}", e))?;
    let ret = unsafe { esp_transport_connect(ws, host.as_ptr(), WS_PORT, CONNECT_TIMEOUT_MS) };

    if ret != 0 {
        unsafe {
            esp_transport_close(ws);
            esp_transport_destroy(ws);
        }
        return Err(anyhow!("esp_transport_connect failed: {}", ret));
    }

    info!("WS connected!");

    // ─── 4. Loop principal: read + write ───

    let mut read_buf = [0u8; 4096];

    loop {
        // ─── Read con timeout corto ───
        //
        // esp_transport_read bloquea hasta leer datos o timeout.
        // - bytes_read > 0: hay datos, procesar según opcode
        // - bytes_read == -1: timeout, no es error (seguimos)
        // - bytes_read < -1: error real, abandonar la conexión
        // - bytes_read == 0: FIN del peer, abandonar

        let bytes_read = unsafe {
            esp_transport_read(
                ws,
                read_buf.as_mut_ptr() as *mut u8,
                read_buf.len() as i32,
                READ_TIMEOUT_MS,
            )
        };

        if bytes_read > 0 {
            // Obtener el opcode del último frame leído.
            let opcode = unsafe { esp_transport_ws_get_read_opcode(ws) as u8 };

            match opcode {
                WS_OPCODE_TEXT => {
                    if let Ok(text) = std::str::from_utf8(&read_buf[..bytes_read as usize]) {
                        handle_text_frame(text, led, override_until);
                    } else {
                        warn!("WS: received non-UTF8 TEXT frame");
                    }
                }
                WS_OPCODE_PING => {
                    info!("WS: ping received, sending pong");
                    // PONG vacío — esperado por el protocolo.
                    unsafe {
                        esp_transport_ws_send_raw(
                            ws,
                            (WS_OPCODE_PONG | WS_OPCODE_FIN) as ws_transport_opcodes_t,
                            ptr::null(),
                            0,
                            CONNECT_TIMEOUT_MS,
                        );
                    }
                }
                WS_OPCODE_CLOSE => {
                    info!("WS: close frame received");
                    break;
                }
                _ => {
                    info!("WS: opcode 0x{:02X} ignored", opcode);
                }
            }
        } else if bytes_read == 0 {
            info!("WS: peer closed connection");
            break;
        } else if bytes_read != -1 {
            // -1 es timeout (normal en polling non-blocking).
            error!("WS read error: {}", bytes_read);
            break;
        }

        // ─── Write: drenar cola outbound ───

        while let Ok(msg) = outbound_rx.try_recv() {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    info!("WS → {}", json);
                    let ret = unsafe {
                        esp_transport_ws_send_raw(
                            ws,
                            WS_OPCODE_TEXT_FIN as ws_transport_opcodes_t,
                            json.as_ptr() as *const u8,
                            json.len() as i32,
                            CONNECT_TIMEOUT_MS,
                        )
                    };
                    if ret < 0 {
                        warn!("WS send failed: {}", ret);
                    }
                }
                Err(e) => warn!("WS: serialize failed: {}", e),
            }
        }

        // Pequeño sleep para no spinnear el CPU.
        thread::sleep(Duration::from_millis(10));
    }

    // ─── 5. Cleanup ───

    unsafe {
        esp_transport_close(ws);
        esp_transport_destroy(ws);
    }

    Ok(())
}

// ─── Handler del frame TEXT (JSON) ───

fn handle_text_frame(
    text: &str,
    led: &Arc<Mutex<LedController<'static>>>,
    override_until: &Arc<Mutex<Option<Instant>>>,
) {
    info!("WS ← {}", text);

    let msg: IncomingMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            warn!("WS: parse error: {} — payload: {}", e, text);
            return;
        }
    };

    match msg {
        IncomingMessage::Hello { device_id } => {
            info!("WS ← Hello echoed: device_id={}", device_id);
        }
        IncomingMessage::Echo { text } => {
            info!("WS ← Echo: {}", text);
        }
        IncomingMessage::SetBrightness { percent } => {
            info!(
                "WS ← SetBrightness: {}% (override {}s)",
                percent, OVERRIDE_DURATION_SECS
            );
            if let Ok(mut l) = led.lock() {
                if let Err(e) = l.set_brightness(percent) {
                    error!("LED set failed: {:?}", e);
                }
            }
            if let Ok(mut g) = override_until.lock() {
                *g = Some(Instant::now() + Duration::from_secs(OVERRIDE_DURATION_SECS));
            }
        }
    }
}
