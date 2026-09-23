// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2026 Jarkko Sakkinen

//! ZMODEM receiver state machine.

use crate::api::{Effect, Input, Progress, SessionEvent};
use crate::buffer::Buffer;
use crate::error::Error;
use crate::file::parse_file_size;
use crate::header::{Encoding, Frame, Header, ZACK_HEADER, ZFIN_HEADER, ZNAK_HEADER, ZRPOS_HEADER};
use crate::io::Read;
use crate::session::{ReceiverEvent, ReceiverPhase, SubpacketPhase};
use crate::string::String;
use crate::wire::{
    write_zrinit, BufferWriter, HeaderReader, RxCrc, SliceReader, SubpacketType,
    SUBPACKET_MAX_SIZE, WIRE_BUF_SIZE,
};
use crate::{zdle, ZDLE};
use core::cmp::min;

const RECEIVER_EVENT_QUEUE_CAP: usize = 4;

/// ZMODEM receiver state machine.
pub struct Receiver {
    state: ReceiverPhase,
    count: u32,
    file_name: String,
    file_size: u32,
    buf: Buffer<SUBPACKET_MAX_SIZE>,
    buf_write_offset: usize,
    data_encoding: Encoding,
    header_reader: HeaderReader,
    subpacket_state: SubpacketPhase,
    subpacket_escape_pending: bool,
    crc: RxCrc,
    outgoing: Buffer<WIRE_BUF_SIZE>,
    outgoing_offset: usize,
    pending_events: [Option<ReceiverEvent>; RECEIVER_EVENT_QUEUE_CAP],
    pending_event_head: usize,
    pending_event_len: usize,
}

impl Receiver {
    /// Create a new receiver instance.
    ///
    /// # Errors
    ///
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    pub fn new() -> Result<Self, Error> {
        let mut receiver = Self {
            state: ReceiverPhase::SessionBegin,
            count: 0,
            file_name: String::new(),
            file_size: 0,
            buf: Buffer::<SUBPACKET_MAX_SIZE>::new(),
            buf_write_offset: 0,
            data_encoding: Encoding::ZBIN,
            header_reader: HeaderReader::new(),
            subpacket_state: SubpacketPhase::Idle,
            subpacket_escape_pending: false,
            crc: RxCrc::new(),
            outgoing: Buffer::<WIRE_BUF_SIZE>::new(),
            outgoing_offset: 0,
            pending_events: [None; RECEIVER_EVENT_QUEUE_CAP],
            pending_event_head: 0,
            pending_event_len: 0,
        };
        receiver.queue_zrinit()?;
        Ok(receiver)
    }

    /// Feeds incoming wire data into the state machine.
    ///
    /// Returns the number of bytes consumed.
    ///
    /// # Errors
    ///
    /// * [`Read`](crate::Error::Read) when the read I/O fails with the serial port
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    /// * [`UnexpectedCrc16`](crate::Error::UnexpectedCrc16) or
    ///   [`UnexpectedCrc32`](crate::Error::UnexpectedCrc32) when corrupted data has been detected
    pub fn feed_incoming(&mut self, input: &[u8]) -> Result<usize, Error> {
        let mut reader = SliceReader::new(input);

        loop {
            if self.outgoing() || !self.drain_file().is_empty() || self.pending_events_full() {
                break;
            }

            let before = reader.consumed();

            if matches!(
                self.state,
                ReceiverPhase::FileReadingSubpacket | ReceiverPhase::FileReadingMetadata
            ) {
                match self.process_subpacket(&mut reader) {
                    Ok(Some(())) => {
                        if self.outgoing()
                            || !self.drain_file().is_empty()
                            || self.pending_events_full()
                        {
                            break;
                        }
                        if reader.consumed() == before {
                            break;
                        }
                        continue;
                    }
                    Ok(None) => break,
                    Err(e) => return Err(e),
                }
            }

            let header = match self.header_reader.read(&mut reader) {
                Ok(Some(header)) => header,
                Ok(None) => break,
                Err(e) => {
                    let _ = self.queue_nak();
                    return Err(e);
                }
            };

            self.handle_header(header)?;

            if self.pending_events_full() {
                break;
            }

            if reader.consumed() == before || reader.consumed() == input.len() {
                break;
            }
        }

        Ok(reader.consumed())
    }

    /// Returns pending outgoing bytes.
    #[must_use]
    pub fn drain_outgoing(&self) -> &[u8] {
        &self.outgoing[self.outgoing_offset..]
    }

    /// Advances the outgoing cursor by `n` bytes.
    pub fn advance_outgoing(&mut self, n: usize) {
        let remaining = self.outgoing.len().saturating_sub(self.outgoing_offset);
        let n = min(n, remaining);
        self.outgoing_offset += n;
        if self.outgoing_offset >= self.outgoing.len() {
            self.outgoing.clear();
            self.outgoing_offset = 0;
        }
    }

    /// Returns pending file data bytes.
    #[must_use]
    pub fn drain_file(&self) -> &[u8] {
        match self.subpacket_state {
            SubpacketPhase::Writing(_) => &self.buf[self.buf_write_offset..],
            _ => &[],
        }
    }

    /// Advances the file output cursor by `n` bytes.
    ///
    /// # Errors
    ///
    /// * [`Write`](crate::Error::Write) when the write I/O fails with the serial port
    pub fn advance_file(&mut self, n: usize) -> Result<(), Error> {
        let SubpacketPhase::Writing(packet) = self.subpacket_state else {
            return Ok(());
        };

        let remaining = self.buf.len().saturating_sub(self.buf_write_offset);
        let n = min(n, remaining);
        self.buf_write_offset = self
            .buf_write_offset
            .checked_add(n)
            .ok_or(Error::OutOfMemory)?;

        if self.buf_write_offset < self.buf.len() {
            return Ok(());
        }

        self.finish_subpacket(packet)
    }

    /// Returns the next pending receiver event.
    pub fn poll_event(&mut self) -> Option<ReceiverEvent> {
        self.pop_event()
    }

    /// Advances the receiver with one 0.6 step/effect API input.
    ///
    /// # Errors
    ///
    /// Returns protocol, state, and I/O errors from the underlying receiver.
    pub fn step<'a>(&'a mut self, input: Input<'a>) -> Result<Progress<'a>, Error> {
        match input {
            Input::Wire(bytes) => {
                let consumed = self.feed_incoming(bytes)?;
                if consumed == 0 {
                    Ok(self.next_progress())
                } else {
                    Ok(Progress::Consumed(consumed))
                }
            }
            Input::OutgoingAdvanced(count) => {
                self.advance_outgoing(count);
                Ok(self.next_progress())
            }
            Input::FileAdvanced(count) => {
                self.advance_file(count)?;
                Ok(self.next_progress())
            }
            Input::Timeout
                if matches!(
                    self.state,
                    ReceiverPhase::SessionBegin | ReceiverPhase::FileBegin
                ) && !self.outgoing() =>
            {
                self.queue_zrinit()?;
                Ok(self.next_progress())
            }
            Input::Abort => {
                self.state = ReceiverPhase::SessionEnd;
                self.push_event(ReceiverEvent::Aborted)?;
                Ok(self.next_progress())
            }
            Input::StartFile(_) | Input::FileData(_) | Input::Timeout | Input::Finish => {
                Err(Error::InvalidState)
            }
        }
    }

    fn next_progress(&mut self) -> Progress<'_> {
        if let Some(event) = self.poll_event() {
            return Progress::Effect(Effect::Event(match event {
                ReceiverEvent::FileStart => SessionEvent::FileStarted(crate::FileInfo::new(
                    self.file_name(),
                    Some(crate::Position::new(self.file_size())),
                )),
                ReceiverEvent::FileComplete => SessionEvent::FileCompleted,
                ReceiverEvent::SessionComplete => SessionEvent::SessionCompleted,
                ReceiverEvent::Aborted => SessionEvent::Aborted,
            }));
        }

        if self.outgoing() {
            return Progress::Effect(Effect::WriteWire(self.drain_outgoing()));
        }

        if !self.drain_file().is_empty() {
            return Progress::Effect(Effect::WriteFile(self.drain_file()));
        }

        Progress::Idle
    }

    #[must_use]
    pub fn file_name(&self) -> &[u8] {
        &self.file_name
    }

    #[must_use]
    pub fn file_size(&self) -> u32 {
        self.file_size
    }

    fn outgoing(&self) -> bool {
        self.outgoing_offset < self.outgoing.len()
    }

    fn pending_events_full(&self) -> bool {
        self.pending_event_len >= RECEIVER_EVENT_QUEUE_CAP
    }

    fn push_event(&mut self, event: ReceiverEvent) -> Result<(), Error> {
        if self.pending_events_full() {
            return Err(Error::OutOfMemory);
        }
        let index = (self.pending_event_head + self.pending_event_len) % RECEIVER_EVENT_QUEUE_CAP;
        self.pending_events[index] = Some(event);
        self.pending_event_len += 1;
        Ok(())
    }

    fn pop_event(&mut self) -> Option<ReceiverEvent> {
        if self.pending_event_len == 0 {
            return None;
        }
        let event = self.pending_events[self.pending_event_head].take();
        self.pending_event_head = (self.pending_event_head + 1) % RECEIVER_EVENT_QUEUE_CAP;
        self.pending_event_len -= 1;
        event
    }

    fn queue_writer(&mut self) -> Result<BufferWriter<'_, WIRE_BUF_SIZE>, Error> {
        if self.outgoing() {
            return Err(Error::Backpressure);
        }
        Ok(BufferWriter::new(&mut self.outgoing))
    }

    fn queue_zrinit(&mut self) -> Result<(), Error> {
        let mut writer = self.queue_writer()?;
        if write_zrinit(&mut writer)?.is_none() {
            return Err(Error::OutOfMemory);
        }
        Ok(())
    }

    fn queue_zrpos(&mut self, count: u32) -> Result<(), Error> {
        let mut writer = self.queue_writer()?;
        if ZRPOS_HEADER.with_count(count).write(&mut writer)?.is_none() {
            return Err(Error::OutOfMemory);
        }
        Ok(())
    }

    fn queue_zack(&mut self) -> Result<(), Error> {
        let count = self.count;
        let mut writer = self.queue_writer()?;
        if ZACK_HEADER.with_count(count).write(&mut writer)?.is_none() {
            return Err(Error::OutOfMemory);
        }
        Ok(())
    }

    fn queue_zfin(&mut self) -> Result<(), Error> {
        let mut writer = self.queue_writer()?;
        if ZFIN_HEADER.write(&mut writer)?.is_none() {
            return Err(Error::OutOfMemory);
        }
        Ok(())
    }

    fn queue_nak(&mut self) -> Result<(), Error> {
        let mut writer = self.queue_writer()?;
        if ZNAK_HEADER.write(&mut writer)?.is_none() {
            return Err(Error::OutOfMemory);
        }
        Ok(())
    }

    fn handle_header(&mut self, header: Header) -> Result<(), Error> {
        match header.frame() {
            Frame::ZRQINIT | Frame::ZDATA if self.state == ReceiverPhase::SessionBegin => {
                self.queue_zrinit()?;
            }
            Frame::ZFILE
                if matches!(
                    self.state,
                    ReceiverPhase::SessionBegin | ReceiverPhase::FileBegin
                ) =>
            {
                self.data_encoding = header.encoding();
                self.state = ReceiverPhase::FileReadingMetadata;
                self.subpacket_state = SubpacketPhase::Reading;
                self.subpacket_escape_pending = false;
                self.crc.reset();
                self.buf.clear();
                self.buf_write_offset = 0;
            }
            Frame::ZDATA
                if matches!(
                    self.state,
                    ReceiverPhase::FileBegin | ReceiverPhase::FileWaitingSubpacket
                ) =>
            {
                if header.count() != self.count {
                    self.queue_zrpos(self.count)?;
                    return Ok(());
                }
                self.data_encoding = header.encoding();
                self.state = ReceiverPhase::FileReadingSubpacket;
                self.subpacket_state = SubpacketPhase::Reading;
                self.subpacket_escape_pending = false;
                self.crc.reset();
                self.buf.clear();
                self.buf_write_offset = 0;
            }
            Frame::ZEOF
                if self.state == ReceiverPhase::FileWaitingSubpacket
                    && header.count() == self.count =>
            {
                self.queue_zrinit()?;
                self.state = ReceiverPhase::FileBegin;
                self.push_event(ReceiverEvent::FileComplete)?;
            }
            Frame::ZABORT | Frame::ZCAN => {
                self.state = ReceiverPhase::SessionEnd;
                self.push_event(ReceiverEvent::Aborted)?;
            }
            Frame::ZFIN
                if matches!(
                    self.state,
                    ReceiverPhase::FileWaitingSubpacket | ReceiverPhase::FileBegin
                ) =>
            {
                self.queue_zfin()?;
                self.state = ReceiverPhase::SessionEnd;
                self.push_event(ReceiverEvent::SessionComplete)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Parses the file info buffer after a ZFILE subpacket is received.
    fn parse_zfile_buf(&mut self) -> Result<(), Error> {
        let payload = &self.buf;
        let mut fields = payload.split(|&b| b == b'\0');

        let file_name_bytes = fields.next().ok_or(Error::MalformedFileName)?;
        if file_name_bytes.is_empty() {
            return Err(Error::MalformedFileName);
        }

        self.file_name.clear();
        self.file_name
            .extend_from_slice(file_name_bytes)
            .map_err(|_| Error::OutOfMemory)?;

        if let Some(size_str_bytes) = fields.next() {
            let size_field_bytes = size_str_bytes
                .split(|&b| b == b' ')
                .next()
                .unwrap_or_default();

            self.file_size = parse_file_size(size_field_bytes)?;
        } else {
            self.file_size = 0;
        }

        self.count = 0;
        Ok(())
    }

    /// Handles reading a single byte for the `SubpacketPhase::Reading` state.
    fn receive_subpacket_data_byte<P>(&mut self, port: &mut P) -> Result<Option<()>, Error>
    where
        P: Read + ?Sized,
    {
        let handle_followup = |this: &mut Self, byte: u8| -> Result<Option<()>, Error> {
            if let Ok(packet) = SubpacketType::try_from(byte) {
                this.crc.update(packet as u8, this.data_encoding);
                this.subpacket_state = SubpacketPhase::Crc(packet);
            } else {
                let unescaped = zdle::UNZDLE_TABLE[byte as usize];
                this.buf.push(unescaped).map_err(|_| Error::OutOfMemory)?;
                this.crc.update(unescaped, this.data_encoding);
            }
            Ok(Some(()))
        };

        if self.subpacket_escape_pending {
            let Some(byte) = port.read_byte()? else {
                return Ok(None);
            };
            self.subpacket_escape_pending = false;
            return handle_followup(self, byte);
        }

        let Some(byte) = port.read_byte()? else {
            return Ok(None);
        };
        if byte == ZDLE {
            let Some(next) = port.read_byte()? else {
                self.subpacket_escape_pending = true;
                return Ok(None);
            };
            return handle_followup(self, next);
        }

        self.buf.push(byte).map_err(|_| Error::OutOfMemory)?;
        self.crc.update(byte, self.data_encoding);
        Ok(Some(()))
    }

    fn process_subpacket<P>(&mut self, port: &mut P) -> Result<Option<()>, Error>
    where
        P: Read + ?Sized,
    {
        match self.subpacket_state {
            SubpacketPhase::Reading => self.receive_subpacket_data_byte(port),
            SubpacketPhase::Crc(packet) => {
                if self.crc.process(port, self.data_encoding)?.is_none() {
                    return Ok(None);
                }

                if self.state == ReceiverPhase::FileReadingMetadata {
                    self.parse_zfile_buf()?;
                    self.buf.clear();
                    self.buf_write_offset = 0;
                    self.crc.reset();
                    self.subpacket_escape_pending = false;

                    self.queue_zrpos(0)?;

                    self.state = ReceiverPhase::FileBegin;
                    self.subpacket_state = SubpacketPhase::Idle;
                    self.push_event(ReceiverEvent::FileStart)?;
                } else {
                    self.subpacket_state = SubpacketPhase::Writing(packet);
                    self.buf_write_offset = 0;
                    if self.buf.is_empty() {
                        self.finish_subpacket(packet)?;
                    }
                }
                Ok(Some(()))
            }
            SubpacketPhase::Writing(_) => Ok(Some(())),
            SubpacketPhase::Idle => Err(Error::InvalidState),
        }
    }

    fn finish_subpacket(&mut self, packet: SubpacketType) -> Result<(), Error> {
        self.count += u32::try_from(self.buf.len()).map_err(|_| Error::OutOfMemory)?;
        self.buf.clear();
        self.buf_write_offset = 0;
        self.crc.reset();

        match packet {
            SubpacketType::ZCRCW => {
                self.queue_zack()?;
                self.state = ReceiverPhase::FileWaitingSubpacket;
                self.subpacket_state = SubpacketPhase::Idle;
                self.subpacket_escape_pending = false;
            }
            SubpacketType::ZCRCQ => {
                self.queue_zack()?;
                self.subpacket_state = SubpacketPhase::Reading;
                self.subpacket_escape_pending = false;
            }
            SubpacketType::ZCRCG => {
                self.subpacket_state = SubpacketPhase::Reading;
                self.subpacket_escape_pending = false;
            }
            SubpacketType::ZCRCE => {
                self.state = ReceiverPhase::FileWaitingSubpacket;
                self.subpacket_state = SubpacketPhase::Idle;
                self.subpacket_escape_pending = false;
            }
        }
        Ok(())
    }
}
