use std::io::{self, Write};

pub fn send(level: tracing::Level, message: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::{os::unix::net::UnixDatagram, sync::Mutex};
        static SOCKET: Mutex<Option<UnixDatagram>> = Mutex::new(None);
        let mut socket = SOCKET.lock().unwrap();
        if socket.is_none() {
            let candidate = UnixDatagram::unbound()?;
            candidate.set_nonblocking(true)?;
            candidate.connect(if cfg!(target_os = "macos") {
                "/var/run/syslog"
            } else {
                "/dev/log"
            })?;
            *socket = Some(candidate);
        }
        let priority = 3 * 8
            + match level {
                tracing::Level::ERROR => 3,
                tracing::Level::WARN => 4,
                tracing::Level::INFO => 6,
                _ => 7,
            };
        let line = format!(
            "<{priority}>smartdns[{}]: {}",
            std::process::id(),
            message.trim_end()
        );
        match socket.as_ref().unwrap().send(line.as_bytes()) {
            Ok(_) => Ok(()),
            Err(error) => {
                *socket = None;
                Err(error)
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (level, message);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "syslog requires a Unix system logger",
        ))
    }
}

#[derive(Clone, Copy)]
pub struct SyslogWriter;

pub struct SyslogRecord {
    level: tracing::Level,
    bytes: Vec<u8>,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SyslogWriter {
    type Writer = SyslogRecord;
    fn make_writer(&'a self) -> Self::Writer {
        SyslogRecord {
            level: tracing::Level::INFO,
            bytes: vec![],
        }
    }
    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        SyslogRecord {
            level: *meta.level(),
            bytes: vec![],
        }
    }
}

impl Write for SyslogRecord {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for SyslogRecord {
    fn drop(&mut self) {
        let _ = send(self.level, &String::from_utf8_lossy(&self.bytes));
    }
}
