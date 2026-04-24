# Rust Embedded desde Cero

## paso-04-websocket

[![ESP32 CI](https://github.com/FMFigueroa/paso-04-websocket/actions/workflows/rust_ci.yml/badge.svg)](https://github.com/FMFigueroa/paso-04-websocket/actions/workflows/rust_ci.yml)

<p align="center">
  <img src="docs/rust-board.png" alt="ESP32-C3-DevKit-RUST-1" width="600">
</p>

Cliente WebSocket (WSS con TLS) sobre el stack heredado de paso-03. El device abre una conexión persistente a un servidor de eco público, manda un JSON de saludo, y queda escuchando comandos remotos tipo `SetBrightness`. Cuando llega uno, el LED cambia inmediatamente y queda fijo en ese brillo durante 3 s antes de volver al ciclo de respiración.

## Qué hace este paso

1. **First boot (sin credenciales):** provisioning vía SoftAP (heredado intacto de paso-02)
2. **Boots siguientes (provisionado):** lee credenciales → conecta a WiFi → abre conexión WSS con `wss://ws.postman-echo.com/raw` → manda `{"type":"Hello","device_id":"..."}` → el LED respira en loop
3. **Cuando llega un comando WS:** el LED adopta el brillo remoto por 3 s y luego retoma la respiración desde donde iba

## Pre-Requisitos

```bash
rustup --version          # Rust (nightly)
cmake --version           # Build system para ESP-IDF
ninja --version           # Backend de compilación
espflash --version        # Herramienta de flash
which ldproxy             # Linker proxy
```

Si falta algo:

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Dependencias (macOS)
brew install cmake ninja python3

# Target RISC-V
rustup target add riscv32imc-unknown-none-elf

# Herramientas de flash y linkeo
cargo install espflash cargo-espflash ldproxy
```

### Hardware — el mismo que paso-03

Ninguno. El LED RGB on-board del DevKit (GPIO2, manejado via RMT) se reusa sin cambios respecto a paso-03.

### Backend — opciones para probar

Por default el firmware apunta a **`wss://ws.postman-echo.com/raw`** — un echo server público con TLS (no requiere setup). Si querés mandarle comandos al device desde tu máquina, necesitás una herramienta WebSocket como [websocat](https://github.com/vi/websocat):

```bash
# macOS
brew install websocat
```

## Limpieza previa (recomendado)

Paso-04 agrega el componente WebSocket client al `sdkconfig.defaults`, así que hay que regenerar el SDK de C:

```bash
espflash erase-flash       # Borra toda la flash (4MB a ceros)
cargo clean                # Limpia cache del build y sdkconfig generado
```

> `cargo clean` es **obligatorio** al venir de paso-03: el archivo `sdkconfig` generado por ESP-IDF se cachea en `target/` y no se actualiza automáticamente si solo cambia `sdkconfig.defaults`.

## Compilar y flashear

```bash
cargo espflash flash --release --monitor
```

El primer build tarda porque recompila el SDK de C con el componente `esp_websocket_client` activado.

## Probar desde tu terminal

Una vez que el device esté provisionado, conectado a WiFi, y el log muestre `WS: connected`, abrí otra terminal:

```bash
websocat wss://ws.postman-echo.com/raw
```

Esto abre una conexión al mismo echo server. Todo lo que escribas se va a ver reflejado por el server, y el device (que está conectado también) va a recibir los mensajes. Probá:

```json
{"type": "SetBrightness", "percent": 90}
```

El LED debería saltar al 90 % inmediatamente, mantenerse 3 s, y después retomar la respiración.

> **Importante:** postman-echo reenvía a **todos** los clientes conectados. Si hay mucho tráfico al mismo tiempo podrías ver mensajes ajenos — es parte del experimento. Para un test aislado, levantá un echo local con `websocat -s 127.0.0.1:8080` y cambiá `WS_URL` en `src/main.rs`.

Comandos JSON aceptados:

```json
{"type": "Hello", "device_id": "test"}
{"type": "Echo", "text": "hola"}
{"type": "SetBrightness", "percent": 50}
```

## Estructura

```
.cargo/config.toml         # Cross-compilation para riscv32imc-esp-espidf
Cargo.toml                 # Dependencias: + serde + serde_json (nuevas)
rust-toolchain.toml        # Nightly + rust-src (build-std)
build.rs                   # Integración con ESP-IDF via embuild
sdkconfig.defaults         # + CONFIG_ESP_WEBSOCKET_CLIENT_ENABLE=y
src/
  main.rs                  # Provisioning → WiFi → WS client + loop con override
  ws_client.rs             # WsClient struct + thread de fondo (NUEVO en paso 4)
  led.rs                   # LedController — WS2812 via RMT (heredado intacto de paso-03)
  wifi.rs                  # Conexión WiFi Station (heredado)
  secure_storage.rs        # NVS wrapper con Zeroize (heredado)
  provisioning.rs          # SoftAP + portal HTTP (heredado)
```

## Dependencias

| Crate          | Uso                                                |
| -------------- | -------------------------------------------------- |
| `esp-idf-hal`  | Hardware Abstraction Layer (GPIO, RMT, modem)      |
| `esp-idf-svc`  | Servicios: WiFi, HTTP, NVS, **WebSocket**, logger  |
| `embedded-svc` | Traits de servicios embedded (HTTP, ipv4)          |
| `heapless`     | Strings de tamaño fijo para config WiFi            |
| `zeroize`      | Borrado seguro de credenciales en memoria          |
| `log`          | Facade de logging (info!, error!, warn!)           |
| `anyhow`       | Manejo de errores con contexto                     |
| **`serde`**    | **Derive de Serialize/Deserialize para los enums de mensajes WS** |
| **`serde_json`** | **Backend JSON — parse/emit de mensajes en el wire** |

## Documentacion

Te invito a unirte a nuestro servidor de Discord para que no te pierdas el desarrollo completo del curso **Rust - Embedded desde Cero**. Encontraras documentacion detallada de cada paso, explicaciones profundas de conceptos, cuestionarios y soporte directo.

<a href="https://discord.gg/dYrqe9HZfz">
  <img alt="Discord" width="35px" src="https://img.icons8.com/external-justicon-lineal-color-justicon/64/external-discord-social-media-justicon-lineal-color-justicon.png"/>
</a>&ensp;
<a href="https://discord.gg/dYrqe9HZfz"><strong>Unirse al servidor — Curso Rust Embedded</strong></a>

## Roadmap

> Este repo es el **Paso 4** del curso **Rust Embedded desde Cero**.

- [Paso 1 — Scaffold del proyecto](https://github.com/FMFigueroa/paso-01-scaffold)
- [Paso 2 — WiFi Station](https://github.com/FMFigueroa/paso-02-wifi-station)
- [Paso 3 — LED PWM](https://github.com/FMFigueroa/paso-03-led-pwm)
- **[Paso 4 — WebSocket Client](https://github.com/FMFigueroa/paso-04-websocket)** ← _este repo_
- [Paso 5 — Light State Management](https://github.com/FMFigueroa/paso-05-light-state)
- [Paso 6 — Telemetria](https://github.com/FMFigueroa/paso-06-telemetry)
- [Paso 7 — Time Sync (SNTP)](https://github.com/FMFigueroa/paso-07-time-sync)
- [Paso 8 — Schedule & Auto Mode](https://github.com/FMFigueroa/paso-08-schedule)
- [Paso 9 — Concurrencia & Watchdog](https://github.com/FMFigueroa/paso-09-watchdog)


## Licencia

[MIT](LICENSE)
