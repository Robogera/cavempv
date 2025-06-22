#![feature(linked_list_cursors)]
mod settings;
use anyhow::Result;
use anyhow::anyhow;
use bytes::BytesMut;
use ftail::Ftail;
use futures::stream::StreamExt;
use libmpv::Format;
use libmpv::events::Event;
use libmpv::{FileState, Mpv};
use log::{LevelFilter, error, info};
use settings::Fragment;
use settings::Settings;
use std::collections::LinkedList;
use std::env::current_dir;
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;
use tokio_util::codec::{Decoder, Encoder};

#[derive(Debug)]
enum Command {
    Next,
    Prev,
    Sleep,
}

struct LineCodec;

impl Decoder for LineCodec {
    type Item = Command;

    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let newline = src.as_ref().iter().position(|b| *b == b'\n');

        if let Some(n) = newline {
            let line = src.split_to(n + 1);
            if let Some(command) = line.get(line.len() - 2) {
                return match command {
                    b'n' => Ok(Some(Command::Next)),
                    b'p' => Ok(Some(Command::Prev)),
                    _ => Ok(None),
                };
            }
        }
        Ok(None)
    }
}

impl Encoder<String> for LineCodec {
    type Error = std::io::Error;

    fn encode(&mut self, _item: String, _dst: &mut BytesMut) -> Result<(), Self::Error> {
        Ok(())
    }
}

trait PlaylistAdder {
    fn replace(&self, path: &str, inf_loop: bool);
    fn queue(&self, path: &str, inf_loop: bool);
}

impl PlaylistAdder for Mpv {
    fn replace(&self, path: &str, inf_loop: bool) {
        self.command(
            "loadfile",
            &[
                path,
                "replace",
                "0",
                if inf_loop { "loop-file=inf" } else { "" },
            ],
        )
        .expect("to replace");
    }
    fn queue(&self, path: &str, inf_loop: bool) {
        self.command(
            "loadfile",
            &[
                path,
                "append-play",
                "0",
                if inf_loop { "loop-file=inf" } else { "" },
            ],
        )
        .expect("to queue");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let s = Settings::new()?;

    Ftail::new()
        .formatted_console(LevelFilter::Debug)
        .daily_file(std::path::Path::new(&s.log_dir), LevelFilter::Info)
        .max_file_size(100)
        .retention_days(7)
        .init()?;

    let mut port = tokio_serial::new(s.serial_port, 57600).open_native_async()?;
    port.set_exclusive(false)?;

    let mut reader = LineCodec.framed(port);

    let (tx, mut rx) = mpsc::channel(1);

    tokio::spawn(async move {
        let mpv = Mpv::new().expect("to start mpv");

        let mut playlist = LinkedList::new();

        s.playlist
            .iter()
            .rev()
            .for_each(|frag| playlist.push_front(frag.clone()));

        let mut cursor = playlist.cursor_front();

        mpv.set_property("audio-device", "pipewire/combined")
            .expect("to set launch options");

        mpv.queue(&cursor.current().unwrap().static_, true);

        while let Some(cmd) = rx.recv().await {
            let mut replaced = false;

            info!("Preparing to play next fragment...");
            if let Some(fadeouts) = &cursor.current().unwrap().fadeout {
                info!("Current fragment has fadeout, processing...");
                let loops = mpv
                    .get_property::<String>("remaining-file-loops")
                    .expect("to get loop count")
                    .trim()
                    .parse::<i32>()
                    .unwrap_or(0);
                info!("Loops left: {loops}");
                let playback_time = mpv
                    .get_property::<String>("playback-time")
                    .expect("to get playback time")
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(0.0);
                let mut maybe_fadeout: Option<&settings::Fadeout> = None;
                if loops == -1 {
                    maybe_fadeout = fadeouts.iter().find(|video| video.before.is_none());
                } else if let Some(fadeout) = fadeouts
                    .iter()
                    .find(|timing| playback_time <= timing.before.unwrap_or(std::f32::MAX))
                {
                    maybe_fadeout = Some(fadeout);
                }
                info!("Playback time: {playback_time}");
                if let Some(fadeout) = maybe_fadeout {
                    info!("Replacing with outro");
                    replaced = true;
                    mpv.replace(&fadeout.video, false);
                    mpv.playlist_clear().expect("to clear playlist");
                }
            }

            info!("Moving playlist position...");

            match cmd {
                Command::Next => {
                    cursor.move_next();
                    cursor.index().or_else(|| {
                        info!("Reached the end of playlist, wrapping over");
                        cursor.move_next();
                        Some(1)
                    });
                }
                Command::Prev => {
                    cursor.move_prev();
                    cursor.index().or_else(|| {
                        info!("Reached the start of playlist, not going further back");
                        cursor.move_next();
                        Some(1)
                    });
                }
                Command::Sleep => {
                    info!("Moving cursor to the start");
                    cursor = playlist.cursor_front();
                }
            };

            if let Some(intro) = &cursor.current().unwrap().intro {
                if replaced {
                    info!("Next fragment has intro. Queuing {intro}");
                    mpv.queue(intro, false);
                } else {
                    info!("Next fragment has intro. Replacing with {intro}");
                    replaced = true;
                    mpv.replace(intro, false);
                    mpv.playlist_clear().expect("to clear playlist");
                }
            }
            let next = &cursor.current().unwrap().static_;
            if replaced {
                info!("Queuing next loop fragment {next}");
                mpv.queue(next, true);
            } else {
                info!("Replacing with  next loop fragment {next}");
                mpv.replace(next, true);
                mpv.playlist_clear().expect("to clear playlist");
            }
        }
    });

    while let Some(line_result) = reader.next().await {
        let line = line_result?;
        if let Err(e) = tx.send(line).await {
            error!("Something's gone terribly wrong: {e:?}");
            return Err(anyhow!(e));
        }
    }

    Ok(())
}
