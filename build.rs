/// Build script para proyectos ESP-IDF.
///
/// `embuild::espidf::sysenv::output()` hace todo el trabajo pesado:
/// 1. Detecta o descarga el ESP-IDF SDK (framework C de Espressif)
/// 2. Configura las variables de entorno para el cross-compiler
/// 3. Genera los bindings de C → Rust
/// 4. Compila el SDK de C y lo linkea con tu código Rust
///
/// Sin este build script, Cargo no sabría cómo compilar para ESP32.
fn main() {
    embuild::espidf::sysenv::output();
}
