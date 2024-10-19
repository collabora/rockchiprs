use std::io::SeekFrom;
use std::{borrow::BorrowMut, task::Poll};

use crate::{
    operation::{OperationSteps, UsbStep},
    protocol::{ChipInfo, FlashId, FlashInfo, ResetOpcode, SECTOR_SIZE},
};
use futures::{future::BoxFuture, ready};
use futures::{AsyncRead, AsyncSeek, AsyncWrite};
use nusb::{
    transfer::{ControlOut, ControlType, Recipient, RequestBuffer},
    DeviceInfo,
};
use thiserror::Error;

/// Error indicate a device is not available
#[derive(Debug, Error)]
#[error("Device is not available: {error}")]
pub struct DeviceUnavalable {
    #[from]
    pub error: nusb::Error,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Usb error: {0}")]
    UsbError(#[from] nusb::Error),
    #[error("Usb transfer error: {0}")]
    UsbTransferError(#[from] nusb::transfer::TransferError),
    #[error("Operation error: {0}")]
    OperationError(#[from] crate::operation::UsbOperationError),
}
type Result<T> = std::result::Result<T, Error>;

/// List rockchip devices
pub fn devices() -> std::result::Result<impl Iterator<Item = DeviceInfo>, nusb::Error> {
    Ok(nusb::list_devices()?.filter(|d| d.vendor_id() == 0x2207))
}

/// nusb based Transport for rockusb operation
pub struct Transport {
    interface: nusb::Interface,
    ep_in: u8,
    ep_out: u8,
}

impl Transport {
    fn new(
        device: nusb::Device,
        interface: u8,
        ep_in: u8,
        ep_out: u8,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        let interface = device.claim_interface(interface)?;
        Ok(Self {
            interface,
            ep_in,
            ep_out,
        })
    }

    /// Create a new transport from a device info
    pub fn from_usb_device_info(
        info: nusb::DeviceInfo,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        let device = info.open()?;
        Self::from_usb_device(device)
    }

    /// Create a new transport from an existing device
    pub fn from_usb_device(device: nusb::Device) -> std::result::Result<Self, DeviceUnavalable> {
        for config in device.clone().configurations() {
            for interface in config.interface_alt_settings() {
                let output = interface.endpoints().find(|e| {
                    e.direction() == nusb::transfer::Direction::Out
                        && e.transfer_type() == nusb::transfer::EndpointType::Bulk
                });
                let input = interface.endpoints().find(|e| {
                    e.direction() == nusb::transfer::Direction::In
                        && e.transfer_type() == nusb::transfer::EndpointType::Bulk
                });

                if let (Some(input), Some(output)) = (input, output) {
                    return Transport::new(
                        device,
                        interface.interface_number(),
                        input.address(),
                        output.address(),
                    );
                }
            }
        }
        Err(DeviceUnavalable {
            error: nusb::Error::new(std::io::ErrorKind::NotFound, "Device not found"),
        })
    }

    /// Convert into an IO object which implements [AsyncRead](futures::io::AsyncRead),
    /// [AsyncWrite](futures::io::AsyncWrite) and [AsyncSeek](futures::io::AsyncSeek)
    pub async fn into_io(self) -> Result<TransportIO> {
        TransportIO::new(self).await
    }

    async fn handle_operation<O, T>(&mut self, mut operation: O) -> Result<T>
    where
        O: OperationSteps<T>,
    {
        loop {
            let step = operation.step();
            match step {
                UsbStep::WriteBulk { data } => {
                    let _written = self
                        .interface
                        .bulk_out(self.ep_out, data.to_vec())
                        .await
                        .into_result()?;
                }
                UsbStep::ReadBulk { data } => {
                    let req = RequestBuffer::new(data.len());
                    let read = self
                        .interface
                        .bulk_in(self.ep_in, req)
                        .await
                        .into_result()?;
                    data.copy_from_slice(&read);
                }
                UsbStep::WriteControl {
                    request_type,
                    request,
                    value,
                    index,
                    data,
                } => {
                    let (control_type, recipient) = (
                        match request_type >> 5 & 0x03 {
                            0 => ControlType::Standard,
                            1 => ControlType::Class,
                            2 => ControlType::Vendor,
                            _ => ControlType::Standard,
                        },
                        match request_type & 0x1f {
                            0 => Recipient::Device,
                            1 => Recipient::Interface,
                            2 => Recipient::Endpoint,
                            3 => Recipient::Other,
                            _ => Recipient::Device,
                        },
                    );
                    let data = ControlOut {
                        control_type,
                        recipient,
                        request,
                        value,
                        index,
                        data,
                    };
                    self.interface.control_out(data).await.into_result()?;
                }
                UsbStep::Finished(r) => break r.map_err(|e| e.into()),
            }
        }
    }

    /// retrieve SoC flash identifier
    pub async fn flash_id(&mut self) -> Result<FlashId> {
        self.handle_operation(crate::operation::flash_id()).await
    }

    /// retrieve SoC flash info
    pub async fn flash_info(&mut self) -> Result<FlashInfo> {
        self.handle_operation(crate::operation::flash_info()).await
    }

    /// retrieve SoC chip info
    pub async fn chip_info(&mut self) -> Result<ChipInfo> {
        self.handle_operation(crate::operation::chip_info()).await
    }

    /// read from the flash
    ///
    /// start_sector with [SECTOR_SIZE](crate::protocol::SECTOR_SIZE) sectors. the data to be read
    /// must be a multiple of [SECTOR_SIZE](crate::protocol::SECTOR_SIZE) bytes
    pub async fn read_lba(&mut self, start_sector: u32, read: &mut [u8]) -> Result<u32> {
        self.handle_operation(crate::operation::read_lba(start_sector, read))
            .await
            .map(|t| t.into())
    }

    /// Create operation to read an lba from the flash
    ///
    /// start_sector based on [SECTOR_SIZE](crate::protocol::SECTOR_SIZE) sectors. the data to be
    /// written must be a multiple of [SECTOR_SIZE](crate::protocol::SECTOR_SIZE) bytes
    pub async fn write_lba(&mut self, start_sector: u32, write: &[u8]) -> Result<u32> {
        self.handle_operation(crate::operation::write_lba(start_sector, write))
            .await
            .map(|t| t.into())
    }

    /// Write a specific area while in maskrom mode; typically 0x471 or 0x472 data as retrieved from a
    /// rockchip boot file
    pub async fn write_maskrom_area(&mut self, area: u16, data: &[u8]) -> Result<()> {
        self.handle_operation(crate::operation::write_area(area, data))
            .await
    }

    /// Reset the device
    pub async fn reset_device(&mut self, opcode: ResetOpcode) -> Result<()> {
        self.handle_operation(crate::operation::reset_device(opcode))
            .await
    }
}

type ReadResult = std::io::Result<(Vec<u8>, usize)>;
enum IoState {
    Idle(Option<TransportIOInner>),
    Read(BoxFuture<'static, (TransportIOInner, ReadResult)>),
    Write(BoxFuture<'static, (TransportIOInner, std::io::Result<usize>)>),
    Flush(BoxFuture<'static, (TransportIOInner, std::io::Result<()>)>),
}

struct TransportIOInner {
    transport: Transport,
    // Read/Write offset in bytes
    offset: u64,
    buffer: Box<[u8; 512]>,
    size: u64,
    // Whether or not the buffer is dirty
    state: BufferState,
}

/// IO object which implements [AsyncRead](futures::io::AsyncRead),
/// [AsyncWrite](futures::io::AsyncWrite) and [AsyncSeek](futures::io::AsyncSeek)
pub struct TransportIO {
    // io execution state
    io_state: IoState,
    size: u64,
}

impl TransportIO {
    /// Create a new IO object around a given transport
    pub async fn new(mut transport: Transport) -> Result<Self> {
        let info = transport.borrow_mut().flash_info().await?;
        let size = info.size();
        let inner = TransportIOInner {
            transport,
            offset: 0,
            buffer: Box::new([0u8; 512]),
            size,
            state: BufferState::Invalid,
        };
        Ok(Self {
            size,
            io_state: IoState::Idle(Some(inner)),
        })
    }

    // Convert into the inner transport
    //
    // Panics if the the TransportIO is currently executing I/O operations
    pub fn into_inner(self) -> Transport {
        let inner = match self.io_state {
            IoState::Idle(Some(i)) => i,
            _ => panic!("TransportIO is currently executing I/O operations"),
        };
        inner.transport
    }

    // Size of the flash in bytes
    pub fn size(&self) -> u64 {
        self.size
    }
}

impl TransportIOInner {
    const MAXIO_SIZE: u64 = 128 * crate::protocol::SECTOR_SIZE;
    fn current_sector(&self) -> u64 {
        self.offset / SECTOR_SIZE
    }

    // Want to start an i/o operation with a given maximum length
    async fn pre_io(&mut self, len: u64) -> std::result::Result<IOOperation, std::io::Error> {
        if self.offset >= self.size {
            return Ok(IOOperation::Eof);
        }

        // Offset inside the current sector
        let sector_offset = self.offset % SECTOR_SIZE;
        // bytes left from current position to end of current sector
        let sector_remaining = SECTOR_SIZE - sector_offset;

        // If the I/O operation is starting at a sector edge and encompasses at least one sector
        // then direct I/O can be done
        if sector_offset == 0 && len >= SECTOR_SIZE {
            // At most read the amount of bytes left
            let left = self.size - self.offset;
            let io_len = len.min(left) / SECTOR_SIZE * SECTOR_SIZE;
            Ok(IOOperation::Direct {
                len: io_len.min(Self::MAXIO_SIZE) as usize,
            })
        } else {
            if self.state == BufferState::Invalid {
                let sector = self.current_sector() as u32;
                self.transport
                    .borrow_mut()
                    .read_lba(sector, self.buffer.as_mut())
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
                self.state = BufferState::Valid;
            }
            Ok(IOOperation::Buffered {
                offset: sector_offset as usize,
                len: len.min(sector_remaining) as usize,
            })
        }
    }

    async fn post_io(&mut self, len: u64) -> std::result::Result<usize, std::io::Error> {
        // Offset inside the current sector
        let sector_offset = self.offset % SECTOR_SIZE;
        // bytes left from current position to end of current sector
        let sector_remaining = SECTOR_SIZE - sector_offset;

        // If going over the sector edge flush the current buffer and invalidate it
        if len >= sector_remaining {
            self.flush_buffer().await?;
            self.state = BufferState::Invalid;
        }
        self.offset += len;
        Ok(len as usize)
    }

    async fn flush_buffer(&mut self) -> std::io::Result<()> {
        if self.state == BufferState::Dirty {
            let sector = self.current_sector() as u32;
            self.transport
                .borrow_mut()
                .write_lba(sector, self.buffer.as_mut())
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            self.state = BufferState::Valid;
        }
        Ok(())
    }

    async fn do_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let sector = self.current_sector() as u32;
        self.transport
            .borrow_mut()
            .read_lba(sector, buf)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        Ok(buf.len())
    }

    async fn do_write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let sector = self.current_sector() as u32;
        self.transport
            .borrow_mut()
            .write_lba(sector, buf)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        Ok(buf.len())
    }
}

enum IOOperation {
    Direct { len: usize },
    Buffered { offset: usize, len: usize },
    Eof,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BufferState {
    // Buffer content doesn't match current offset
    Invalid,
    // Buffer content matches offset and device-side
    Valid,
    // Buffer content matches offset and has outstanding data
    Dirty,
}

impl AsyncWrite for TransportIO {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<futures::io::Result<usize>> {
        let me = self.get_mut();
        loop {
            match me.io_state {
                IoState::Idle(ref mut inner) => {
                    let mut inner = inner.take().unwrap();
                    let buf =
                        Vec::from(&buf[0..buf.len().min(TransportIOInner::MAXIO_SIZE as usize)]);
                    me.io_state = IoState::Write(Box::pin(async move {
                        let io = match inner.pre_io(buf.len() as u64).await {
                            Ok(io) => io,
                            Err(e) => return (inner, Err(e)),
                        };
                        let r = match io {
                            IOOperation::Direct { len } => {
                                match inner.do_write(&buf[..len]).await {
                                    Ok(r) => r,
                                    Err(e) => return (inner, Err(e)),
                                }
                            }
                            IOOperation::Buffered { offset, len } => {
                                inner.buffer[offset..offset + len].copy_from_slice(&buf[0..len]);
                                inner.state = BufferState::Dirty;
                                len
                            }
                            IOOperation::Eof => {
                                return (
                                    inner,
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::Other,
                                        "Trying to write past end of area",
                                    )),
                                )
                            }
                        };
                        let r = inner.post_io(r as u64).await;
                        (inner, r)
                    }))
                }
                IoState::Write(ref mut f) => {
                    let (inner, r) = ready!(f.as_mut().poll(cx));
                    me.io_state = IoState::Idle(Some(inner));
                    return Poll::Ready(r);
                }
                _ => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "Invalid transport state",
                    )))
                }
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<futures::io::Result<()>> {
        let me = self.get_mut();
        loop {
            match me.io_state {
                IoState::Idle(ref mut inner) => {
                    let mut inner = inner.take().unwrap();
                    me.io_state = IoState::Flush(Box::pin(async move {
                        let r = inner.flush_buffer().await;
                        (inner, r)
                    }))
                }
                IoState::Flush(ref mut f) => {
                    let (inner, r) = ready!(f.as_mut().poll(cx));
                    me.io_state = IoState::Idle(Some(inner));
                    return Poll::Ready(r);
                }
                _ => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "Invalid transport state",
                    )))
                }
            }
        }
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<futures::io::Result<()>> {
        self.poll_flush(cx)
    }
}

impl AsyncRead for TransportIO {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<futures::io::Result<usize>> {
        let me = self.get_mut();
        if let IoState::Idle(ref mut inner) = me.io_state {
            let mut inner = inner.take().unwrap();
            let mut buf = vec![0; buf.len()];
            me.io_state = IoState::Read(Box::pin(async move {
                let io = match inner.pre_io(buf.len() as u64).await {
                    Ok(io) => io,
                    Err(e) => return (inner, Err(e)),
                };
                let r = match io {
                    IOOperation::Direct { len } => match inner.do_read(&mut buf[..len]).await {
                        Ok(r) => r,
                        Err(e) => return (inner, Err(e)),
                    },
                    IOOperation::Buffered { offset, len } => {
                        buf[0..len].copy_from_slice(&inner.buffer[offset..offset + len]);
                        len
                    }
                    IOOperation::Eof => 0,
                };
                let r = inner.post_io(r as u64).await.map(|r| (buf, r));
                (inner, r)
            }))
        }

        match me.io_state {
            IoState::Read(ref mut f) => {
                let (inner, r) = ready!(f.as_mut().poll(cx));
                me.io_state = IoState::Idle(Some(inner));
                let r = match r {
                    Ok((read_buf, r)) => {
                        buf[..r].copy_from_slice(&read_buf[..r]);
                        Ok(r)
                    }
                    Err(e) => Err(e),
                };
                Poll::Ready(r)
            }
            _ => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Invalid transport state",
            ))),
        }
    }
}

impl AsyncSeek for TransportIO {
    fn poll_seek(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        pos: SeekFrom,
    ) -> std::task::Poll<futures::io::Result<u64>> {
        let me = self.get_mut();
        match me.io_state {
            IoState::Idle(Some(ref mut inner)) => {
                inner.offset = match pos {
                    SeekFrom::Start(offset) => inner.size.min(offset),
                    SeekFrom::End(offset) => {
                        if offset > 0 {
                            inner.size
                        } else {
                            let offset = offset.unsigned_abs();
                            inner.size.saturating_sub(offset)
                        }
                    }
                    SeekFrom::Current(offset) => {
                        if offset > 0 {
                            let offset = offset as u64;
                            inner.offset.saturating_add(offset).min(inner.size)
                        } else {
                            let offset = offset.unsigned_abs();
                            inner.offset.saturating_sub(offset)
                        }
                    }
                };
                Poll::Ready(Ok(inner.offset))
            }
            _ => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Invalid transport state",
            ))),
        }
    }
}
