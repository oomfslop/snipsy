use base64::Engine;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

pub fn create_handoff(image: &[u8]) -> Result<PathBuf, String> {
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
    let mut file = tempfile::Builder::new()
        .prefix("snipsy-lens-")
        .suffix(".html")
        .tempfile()
        .map_err(|error| error.to_string())?;
    file.write_all(html.as_bytes())
        .map_err(|error| error.to_string())?;
    return file
        .into_temp_path()
        .keep()
        .map_err(|error| error.to_string());
}

pub fn schedule_cleanup(path: PathBuf) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(30));
        let _ = std::fs::remove_file(path);
    });
}
