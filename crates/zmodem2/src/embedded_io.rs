// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Jarkko Sakkinen

//! Adapters for `embedded-io` traits.

use crate::{Error, Read, Seek, String, Write};

/// Adapter that implements zmodem2 I/O traits for an `embedded-io` object.
pub struct EmbeddedIo<T> {
    inner: T,
}

impl<T> EmbeddedIo<T> {
    /// Creates a new adapter.
    #[must_use]
    pub const fn new(inner: T) -> Self {
        Self { inner }
    }

    /// Returns a shared reference to the wrapped object.
    #[must_use]
    pub const fn inner(&self) -> &T {
        &self.inner
    }

    /// Returns a mutable reference to the wrapped object.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Consumes the adapter and returns the wrapped object.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> Read for EmbeddedIo<T>
where
    T: embedded_io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<Option<u32>, Error> {
        match embedded_io::Read::read(&mut self.inner, buf) {
            Ok(bytes_read) => u32::try_from(bytes_read)
                .map(Some)
                .map_err(|_| Error::OutOfMemory),
            Err(error) => Err(read_error(error)),
        }
    }

    fn read_byte(&mut self) -> Result<Option<u8>, Error> {
        let mut byte = [0u8; 1];
        match embedded_io::Read::read(&mut self.inner, &mut byte) {
            Ok(0) => Err(Error::UnexpectedEof),
            Ok(_) => Ok(Some(byte[0])),
            Err(error) => Err(read_error(error)),
        }
    }
}

impl<T> Write for EmbeddedIo<T>
where
    T: embedded_io::Write,
{
    fn write(&mut self, buf: &[u8]) -> Result<Option<u32>, Error> {
        match embedded_io::Write::write(&mut self.inner, buf) {
            Ok(bytes_written) => u32::try_from(bytes_written)
                .map(Some)
                .map_err(|_| Error::OutOfMemory),
            Err(error) => Err(write_error(error)),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<Option<()>, Error> {
        embedded_io::Write::write_all(&mut self.inner, buf)
            .map(Some)
            .map_err(write_error)
    }
}

impl<T> Seek for EmbeddedIo<T>
where
    T: embedded_io::Seek,
{
    fn seek(&mut self, offset: u32) -> Result<Option<u32>, Error> {
        match embedded_io::Seek::seek(
            &mut self.inner,
            embedded_io::SeekFrom::Start(u64::from(offset)),
        ) {
            Ok(position) => u32::try_from(position)
                .map(Some)
                .map_err(|_| Error::UnexpectedEof),
            Err(error) => Err(read_error(error)),
        }
    }
}

fn read_error<E>(_error: E) -> Error
where
    E: embedded_io::Error,
{
    Error::Read(String::from("embedded-io read error"))
}

fn write_error<E>(_error: E) -> Error
where
    E: embedded_io::Error,
{
    Error::Write(String::from("embedded-io write error"))
}
