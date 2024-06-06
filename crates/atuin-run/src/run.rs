/// Create and manage pseudoterminals
use std::io::{Read, Write};

use eyre::{eyre, Result};

use bytes::Bytes;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub struct Pty {
    tx: Sender<Bytes>,
    master: Box<dyn MasterPty>,
}

impl Pty {
    pub async fn open_shell<'a>(rows: u16, cols: u16, shell: &str, dir: &str) -> Result<Self> {
        let sys = native_pty_system();

        let pair = sys
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| eyre!("Failed to open pty: {}", e))?;

        let cmd = CommandBuilder::new(shell);

        tokio::task::spawn_blocking(move || {
            let mut child = pair.slave.spawn_command(cmd).unwrap();

            // Wait for the child to exit
            let _ = child.wait().unwrap();

            // Ensure slave is dropped
            // This closes file handles, we can deadlock if this is not done correctly.
            drop(pair.slave);
        });

        // Handle input -> write to master writer
        let (master_tx, mut master_rx) = channel::<Bytes>(32);

        let mut writer = pair.master.take_writer().unwrap();

        tokio::spawn(async move {
            while let Some(bytes) = master_rx.recv().await {
                writer.write_all(&bytes).unwrap();
                writer.flush().unwrap();
            }

            // When the channel has been closed, we won't be getting any more input. Close the
            // writer and the master.
            // This will also close the writer, which sends EOF to the underlying shell. Ensuring
            // that is also closed.
            drop(writer);
        });

        Ok(Pty {
            tx: master_tx,
            master: pair.master,
        })
    }

    pub async fn send_bytes(&self, bytes: Bytes) -> Result<()> {
        self.tx
            .send(bytes)
            .await
            .map_err(|e| eyre!("Failed to write to master tx: {}", e))
    }

    pub async fn send_string(&self, cmd: &str) -> Result<()> {
        let bytes: Vec<u8> = cmd.bytes().collect();
        let bytes = Bytes::from(bytes);

        self.send_bytes(bytes).await
    }

    pub async fn send_single_string(&self, cmd: &str) -> Result<()> {
        let mut bytes: Vec<u8> = cmd.bytes().collect();
        bytes.push(0x04);

        let bytes = Bytes::from(bytes);

        self.send_bytes(bytes).await
    }

    pub fn reader(&self) -> Result<Box<dyn Read + Send>> {
        self.master
            .try_clone_reader()
            .map_err(|e| eyre!("Failed to clone master reader: {}", e))
    }
}
