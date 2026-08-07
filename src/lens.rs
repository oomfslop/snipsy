use base64::Engine;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn upload_image_and_open_results(image_path: &Path) {
    let image = match std::fs::read(image_path) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("could not read Lens image: {error}");
            return;
        }
    };
    let html = r##"<!doctype html>
<meta charset="utf-8">
<title>Opening Google Lens…</title>
<form id="form" method="post" enctype="multipart/form-data" action="https://lens.google.com/v3/upload?ep=gsbubb&authuser=0&hl=en">
  <input id="image" type="file" name="encoded_image">
</form>
<script>
  const bytes = Uint8Array.from(atob("__IMAGE_BASE64__"), character => character.charCodeAt(0));
  const transfer = new DataTransfer();
  transfer.items.add(new File([bytes], "selection.png", { type: "image/png" }));
  document.querySelector("#image").files = transfer.files;
  const form = document.querySelector("#form");
  const action = new URL(form.action);
  action.searchParams.set("st", Date.now());
  action.searchParams.set("vpw", window.screen.width);
  action.searchParams.set("vph", window.screen.height);
  form.action = action;
  form.submit();
</script>
"##
    .replace(
        "__IMAGE_BASE64__",
        &base64::engine::general_purpose::STANDARD.encode(image),
    );
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let html_path = std::env::temp_dir().join(format!(
        "snipsy-lens-{}-{timestamp}.html",
        std::process::id()
    ));
    let mut html_file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&html_path)
    {
        Ok(html_file) => html_file,
        Err(error) => {
            eprintln!("could not create Lens handoff: {error}");
            return;
        }
    };
    if let Err(error) = html_file.write_all(html.as_bytes()) {
        eprintln!("could not write Lens handoff: {error}");
        let _ = std::fs::remove_file(&html_path);
        return;
    }
    drop(html_file);

    match Command::new("/usr/bin/open").arg(&html_path).status() {
        Ok(status) if status.success() => {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(30));
                let _ = std::fs::remove_file(html_path);
            });
        }
        Ok(status) => {
            eprintln!("could not open Lens handoff: {status}");
            let _ = std::fs::remove_file(html_path);
        }
        Err(error) => {
            eprintln!("could not open Lens handoff: {error}");
            let _ = std::fs::remove_file(html_path);
        }
    }
}
