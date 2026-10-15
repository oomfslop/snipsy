use anyhow::Result;
use base64::Engine;
use gpui_kit::App;
use std::{io::Write, path::PathBuf, time::Duration};

/// Google Lens only accepts uploads as a browser form post, so the image goes through a
/// local page that submits itself. The endpoint is undocumented and may change.
pub fn create_handoff(png: &[u8]) -> Result<PathBuf> {
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
        &base64::engine::general_purpose::STANDARD.encode(png),
    );
    let mut file = tempfile::Builder::new()
        .prefix("snipsy-lens-")
        .suffix(".html")
        .tempfile()?;
    file.write_all(html.as_bytes())?;
    return Ok(file.into_temp_path().keep()?);
}

/// The browser reads the page asynchronously, so it cannot be deleted right after opening.
pub fn schedule_cleanup(path: PathBuf, cx: &App) {
    let timer = cx.background_executor().timer(Duration::from_secs(30));
    cx.background_executor()
        .spawn(async move {
            timer.await;
            if let Err(error) = std::fs::remove_file(&path) {
                eprintln!("could not remove {}: {error}", path.display());
            }
        })
        .detach();
}
