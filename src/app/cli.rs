use crate::app::content::parse_color;
use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::{
    ffi::{OsStr, OsString},
    io::{ErrorKind, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const IPC_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Parser, Debug)]
#[command(name = "snipsy", about = "Screenshot editor and floating image pins")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Clone, Debug, PartialEq)]
pub enum Command {
    /// Select a region to capture, annotate, or record
    Snip,
    /// Start or stop a screen recording
    Record,
    /// Pin clipboard content, text, or files
    Paste {
        #[arg(long, num_args = 1.., value_name = "TEXT", conflicts_with = "files")]
        plain: Option<Vec<String>>,
        #[arg(long, num_args = 1.., value_name = "FILE")]
        files: Option<Vec<PathBuf>>,
    },
    /// Draw on a blank full-screen canvas
    Whiteboard {
        #[arg(long, value_parser = parse_color_argument, default_value = "white")]
        color: [u8; 4],
    },
    /// Show or hide every pin
    TogglePins,
    /// Toggle click-through for the pin under the pointer
    ToggleClickThrough,
    Preferences,
    Quit,
}

impl Command {
    pub const PASTE_CLIPBOARD: Self = Self::Paste {
        plain: None,
        files: None,
    };

    fn resolve_paths(mut self, directory: &Path) -> Self {
        if let Self::Paste {
            files: Some(files), ..
        } = &mut self
        {
            for file in files {
                *file = directory.join(&*file);
            }
        }
        return self;
    }
}

fn parse_color_argument(value: &str) -> Result<[u8; 4], String> {
    return parse_color(value).ok_or_else(|| format!("invalid color: {value}"));
}

pub fn connect_instance(data_dir: &Path, args: &[OsString]) -> Result<Option<UnixListener>> {
    let message = encode_message(&std::env::current_dir()?, args);
    return connect_socket(&data_dir.join("snipsy.sock"), &message);
}

fn connect_socket(socket: &Path, message: &[u8]) -> Result<Option<UnixListener>> {
    if message.len() > MAX_MESSAGE_BYTES {
        bail!("instance command is too large");
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(socket.with_extension("lock"))?;
    lock.lock()?;
    match UnixStream::connect(socket) {
        Ok(mut stream) => {
            stream.set_write_timeout(Some(IPC_TIMEOUT))?;
            stream.write_all(message)?;
            return Ok(None);
        }
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) => {}
        Err(error) => return Err(error.into()),
    }
    crate::app::storage::remove_file_if_exists(socket)?;
    return Ok(Some(UnixListener::bind(socket)?));
}

pub fn listen(listener: UnixListener, sender: async_channel::Sender<Command>) -> Result<()> {
    std::thread::Builder::new()
        .name("snipsy-instance".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream
                    .map_err(anyhow::Error::from)
                    .and_then(read_instance_command)
                {
                    Ok(Some(command)) => {
                        if sender.send_blocking(command).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("invalid instance command: {error:#}"),
                }
            }
        })?;
    return Ok(());
}

fn read_instance_command(stream: UnixStream) -> Result<Option<Command>> {
    if let Err(error) = stream.set_read_timeout(Some(IPC_TIMEOUT)) {
        // macOS rejects timeout changes once the peer has closed; reads then hit EOF without blocking.
        if error.kind() != ErrorKind::InvalidInput {
            return Err(error.into());
        }
    }
    let mut input = Vec::new();
    stream
        .take(MAX_MESSAGE_BYTES as u64 + 1)
        .read_to_end(&mut input)?;
    if input.len() > MAX_MESSAGE_BYTES {
        bail!("instance command is too large");
    }
    return decode_message(&input);
}

fn encode_message(directory: &Path, args: &[OsString]) -> Vec<u8> {
    return std::iter::once(directory.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(OsStrExt::as_bytes)
        .collect::<Vec<_>>()
        .join(&0);
}

fn decode_message(input: &[u8]) -> Result<Option<Command>> {
    let mut parts = input.split(|byte| *byte == 0).map(OsStr::from_bytes);
    let Some(directory) = parts.next().filter(|directory| !directory.is_empty()) else {
        bail!("missing working directory");
    };
    let args = std::iter::once(OsString::from("snipsy"))
        .chain(parts.map(|part| OsString::from_vec(part.as_bytes().to_vec())));
    let cli = Cli::try_parse_from(args)?;
    return Ok(cli
        .command
        .map(|command| command.resolve_paths(Path::new(directory))));
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, Command, MAX_MESSAGE_BYTES, connect_socket, decode_message, encode_message,
        read_instance_command,
    };
    use clap::Parser;
    use std::{
        ffi::OsString,
        io::Write,
        os::unix::{
            ffi::OsStringExt,
            net::{UnixListener, UnixStream},
        },
        path::PathBuf,
        sync::Barrier,
    };

    #[test]
    fn instance_reader_accepts_a_command_after_its_sender_closes() {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        sender.write_all(b"/tmp\0snip").unwrap();
        drop(sender);

        assert_eq!(
            read_instance_command(receiver).unwrap(),
            Some(Command::Snip)
        );
    }

    #[test]
    fn instance_reader_rejects_oversized_commands_without_truncating_them() {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        let mut message = b"/tmp\0paste\0--plain\0".to_vec();
        message.resize(MAX_MESSAGE_BYTES + 1, b'a');
        let worker = std::thread::spawn(move || {
            let _ = sender.write_all(&message);
        });

        let result = read_instance_command(receiver);
        worker.join().unwrap();

        assert!(result.unwrap_err().to_string().contains("too large"));
    }

    #[test]
    fn simultaneous_launches_create_only_one_listener() {
        for _ in 0..50 {
            let directory = tempfile::tempdir().unwrap();
            let socket = directory.path().join("snipsy.sock");
            drop(UnixListener::bind(&socket).unwrap());
            let message = encode_message(directory.path(), &["snip".into()]);
            let barrier = Barrier::new(2);
            let results = std::thread::scope(|scope| {
                let workers = [0, 1].map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        connect_socket(&socket, &message).unwrap()
                    })
                });
                workers.map(|worker| worker.join().unwrap())
            });
            let listeners = results.into_iter().flatten().collect::<Vec<_>>();
            assert_eq!(listeners.len(), 1);
            let (stream, _) = listeners[0].accept().unwrap();
            assert_eq!(read_instance_command(stream).unwrap(), Some(Command::Snip));
        }
    }

    fn parse(args: &[&str]) -> Option<Command> {
        return Cli::try_parse_from(std::iter::once("snipsy").chain(args.iter().copied()))
            .unwrap()
            .command;
    }

    #[test]
    fn commands_parse_text_files_and_colors() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&["snip"]), Some(Command::Snip));
        assert_eq!(parse(&["paste"]), Some(Command::PASTE_CLIPBOARD));
        assert_eq!(
            parse(&["paste", "--plain", "hello", "world"]),
            Some(Command::Paste {
                plain: Some(vec!["hello".into(), "world".into()]),
                files: None
            })
        );
        assert_eq!(
            parse(&["whiteboard", "--color", "#01020380"]),
            Some(Command::Whiteboard {
                color: [1, 2, 3, 128]
            })
        );
        assert_eq!(
            parse(&["whiteboard"]),
            Some(Command::Whiteboard { color: [255; 4] })
        );
        assert!(Cli::try_parse_from(["snipsy", "paste", "--files"]).is_err());
        assert!(Cli::try_parse_from(["snipsy", "whiteboard", "--color", "nope"]).is_err());
    }

    #[test]
    fn forwarded_commands_keep_raw_paths_and_resolve_against_the_sender_directory() {
        let raw = OsString::from_vec(vec![0xff, b'a']);
        let message = encode_message(
            &PathBuf::from("/work"),
            &[
                "paste".into(),
                "--files".into(),
                "relative.png".into(),
                "/absolute.png".into(),
                raw.clone(),
            ],
        );
        assert_eq!(
            decode_message(&message).unwrap(),
            Some(Command::Paste {
                plain: None,
                files: Some(vec![
                    PathBuf::from("/work/relative.png"),
                    PathBuf::from("/absolute.png"),
                    PathBuf::from("/work").join(raw),
                ]),
            })
        );
        assert_eq!(
            decode_message(&encode_message(&PathBuf::from("/"), &[])).unwrap(),
            None
        );
        assert!(decode_message(b"").is_err());
    }
}
