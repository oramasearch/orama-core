use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

pub fn list_directory_in_path<P: AsRef<Path>>(p: P) -> Result<Vec<PathBuf>> {
    let mut result = vec![];

    let p: PathBuf = p.as_ref().to_path_buf();

    let entries =
        std::fs::read_dir(&p).with_context(|| format!("Cannot read directory {:?}", p))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("Cannot get entry from dir {:?}", p))?;

        let is_dir = entry
            .file_type()
            .with_context(|| format!("Cannot get file type of {:?}", entry))?
            .is_dir();

        if !is_dir {
            continue;
        }

        let path = entry.path();
        result.push(path);
    }

    Ok(result)
}

pub struct BufferedFile;
impl BufferedFile {
    pub fn create(path: PathBuf) -> Result<WriteBufferedFile> {
        let file = std::fs::File::create_new(&path)
            .with_context(|| format!("Cannot create file at {:?}", path))?;
        let buf = std::io::BufWriter::new(file);
        Ok(WriteBufferedFile {
            path,
            closed: false,
            buf,
        })
    }

    pub fn open<P: AsRef<Path>>(path: P) -> Result<ReadBufferedFile> {
        Ok(ReadBufferedFile {
            path: path.as_ref().to_path_buf(),
        })
    }
}

pub struct ReadBufferedFile {
    path: PathBuf,
}

impl ReadBufferedFile {
    pub fn read_json_data<T: serde::de::DeserializeOwned>(self) -> Result<T> {
        let file = std::fs::File::open(&self.path)
            .with_context(|| format!("Cannot open file at {:?}", self.path))?;
        let reader = std::io::BufReader::new(file);
        let data = serde_json::from_reader(reader)
            .with_context(|| format!("Cannot read json data from {:?}", self.path))?;
        Ok(data)
    }

    pub fn read_bincode_data<T: serde::de::DeserializeOwned>(self) -> Result<T> {
        let file = std::fs::File::open(&self.path)
            .with_context(|| format!("Cannot open file at {:?}", self.path))?;
        let reader = std::io::BufReader::new(file);
        let data = bincode::deserialize_from(reader)
            .with_context(|| format!("Cannot read bincode data from {:?}", self.path))?;
        Ok(data)
    }
}

pub struct WriteBufferedFile {
    path: PathBuf,
    buf: std::io::BufWriter<std::fs::File>,
    closed: bool,
}
impl WriteBufferedFile {
    pub fn write_json_data<T: serde::Serialize>(mut self, data: &T) -> Result<()> {
        serde_json::to_writer(&mut self.buf, data)
            .with_context(|| format!("Cannot write json data to {:?}", self.path))?;
        self.close()
    }

    pub fn write_bincode_data<T: serde::Serialize>(mut self, data: &T) -> Result<()> {
        bincode::serialize_into(&mut self.buf, data)
            .with_context(|| format!("Cannot write bincode data to {:?}", self.path))?;
        self.close()
    }

    pub fn close(mut self) -> Result<()> {
        self.drop_all()
    }

    fn drop_all(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }

        self.closed = true;

        self.buf
            .flush()
            .with_context(|| format!("Cannot flush buffer {:?}", self.path))?;

        let mut inner = self.buf.get_ref();
        inner
            .flush()
            .with_context(|| format!("Cannot flush file {:?}", self.path))?;
        inner
            .sync_all()
            .with_context(|| format!("Cannot sync_all file {:?}", self.path))?;

        Ok(())
    }
}

// Proxy all std::io::Write methods to the inner buffer
impl std::io::Write for WriteBufferedFile {
    fn by_ref(&mut self) -> &mut Self {
        self.buf.by_ref();
        self
    }

    fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        self.buf.write_vectored(bufs)
    }

    fn write_fmt(&mut self, fmt: std::fmt::Arguments<'_>) -> std::io::Result<()> {
        self.buf.write_fmt(fmt)
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.buf.write_all(buf)
    }

    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.buf.flush()
    }
}

impl Drop for WriteBufferedFile {
    fn drop(&mut self) {
        self.drop_all()
            .expect("WriteBufferedFile drop failed. Use `close` method to handle errors.");
    }
}
