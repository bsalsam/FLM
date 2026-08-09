//! Diálogos simples via `zenity`.
//!
//! Optamos por chamar o `zenity` como subprocesso em vez de linkar GTK: a
//! bandeja usa `ksni`, que fala StatusNotifierItem direto no D-Bus e é Rust
//! puro, então o daemon inteiro compila sem nenhum header de GTK instalado
//! (esta máquina não tem `libgtk-3-dev`, e exigi-lo só para três caixinhas de
//! texto encareceria o build de quem for compilar o projeto). O `zenity` já vem
//! no GNOME.
//!
//! Todas as funções bloqueiam esperando o usuário, então devem ser chamadas de
//! uma thread separada -- nunca de dentro de um callback de menu, que roda no
//! loop de serviço do `ksni`.

use std::process::Command;

fn rodar(args: &[&str]) -> Option<String> {
    let saida = Command::new("zenity").args(args).output().ok()?;
    // Código de saída != 0 significa Cancelar/fechar a janela.
    if !saida.status.success() {
        return None;
    }
    let texto = String::from_utf8_lossy(&saida.stdout).trim().to_string();
    Some(texto)
}

/// Caixa de entrada de texto. Devolve `None` se o usuário cancelar.
pub fn entrada(titulo: &str, texto: &str, valor_atual: &str) -> Option<String> {
    let r = rodar(&[
        "--entry",
        "--title",
        titulo,
        "--text",
        texto,
        "--entry-text",
        valor_atual,
    ])?;
    let r = r.trim().to_string();
    if r.is_empty() { None } else { Some(r) }
}

pub fn erro(texto: &str) {
    let _ = Command::new("zenity")
        .args(["--error", "--title", "FLM", "--width", "420", "--text", texto])
        .status();
}

pub fn info(texto: &str) {
    let _ = Command::new("zenity")
        .args(["--info", "--title", "FLM", "--width", "420", "--text", texto])
        .status();
}
