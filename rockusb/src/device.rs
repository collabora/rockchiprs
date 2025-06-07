use std::{
    borrow::BorrowMut,
    io::{Read, Seek, SeekFrom, Write},
    marker::PhantomData,
};

use crate::{
    operation::OperationSteps,
    protocol::{Capability, ChipInfo, FlashId, FlashInfo, ResetOpcode, SECTOR_SIZE},
};
use thiserror::Error;

#[derive(Debug, Clone, Eq, PartialEq, Error)]
/// Error type return by most [Device] method
pub enum Error<TE> {
    #[error("Usb error: {0}")]
    UsbError(TE),
    #[error("Operation error: {0}")]
    OperationError(#[from] crate::operation::UsbOperationError),
}

#[maybe_async_cfg::maybe(sync(keep_self), async(feature = "async"))]
/// Device wrapper for rockusb operations
pub struct Device<Transport> {
    transport: Transport,
}

#[maybe_async_cfg::maybe(sync(keep_self), async(feature = "async"))]
/// Trait to be implemented by backing transports
pub trait Transport {
    type TransportError: Send + Sync + std::fmt::Debug + std::fmt::Display + 'static;
    #[maybe_async_cfg::only_if(sync)]
    fn handle_operation<O, T>(&mut self, operation: O) -> Result<T, Error<Self::TransportError>>
    where
        O: OperationSteps<T>;
    #[maybe_async_cfg::only_if(async)]
    fn handle_operation<O, T>(
        &mut self,
        operation: O,
    ) -> impl Future<Output = Result<T, Error<Self::TransportError>>> + Send
    where
        O: OperationSteps<T> + Send,
        T: Send;
}

/// Result type return by most [Device] method
pub type DeviceResult<T, Trans> = Result<T, Error<<Trans as Transport>::TransportError>>;
#[cfg(feature = "async")]
/// Result type return by most [DeviceAsync] method
pub type DeviceResultAsync<T, Trans> = Result<T, Error<<Trans as TransportAsync>::TransportError>>;

#[maybe_async_cfg::maybe(
    sync(keep_self),
    async(
        feature = "async",
        idents(
            DeviceResult(async = "DeviceResultAsync"),
            Transport(async = "TransportAsync")
        )
    )
)]
impl<T> Device<T>
where
    T: Transport,
{
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Get a reference to the underlying transport
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// retrieve SoC flash identifier
    pub async fn flash_id(&mut self) -> DeviceResult<FlashId, T> {
        self.transport
            .handle_operation(crate::operation::flash_id())
            .await
    }

    /// retrieve SoC flash info
    pub async fn flash_info(&mut self) -> DeviceResult<FlashInfo, T> {
        self.transport
            .handle_operation(crate::operation::flash_info())
            .await
    }

    /// retrieve SoC chip info
    pub async fn chip_info(&mut self) -> DeviceResult<ChipInfo, T> {
        self.transport
            .handle_operation(crate::operation::chip_info())
            .await
    }

    /// retrieve SoC capability
    pub async fn capability(&mut self) -> DeviceResult<Capability, T> {
        self.transport
            .handle_operation(crate::operation::capability())
            .await
    }

    /// read from the flash
    ///
    /// start_sector with [SECTOR_SIZE] sectors. the data to be read
    /// must be a multiple of [SECTOR_SIZE] bytes
    pub async fn read_lba(&mut self, start_sector: u32, read: &mut [u8]) -> DeviceResult<u32, T> {
        self.transport
            .handle_operation(crate::operation::read_lba(start_sector, read))
            .await
            .map(|t| t.into())
    }

    /// Create operation to read an lba from the flash
    ///
    /// start_sector based on [SECTOR_SIZE] sectors. the data to be
    /// written must be a multiple of [SECTOR_SIZE] bytes
    pub async fn write_lba(&mut self, start_sector: u32, write: &[u8]) -> DeviceResult<u32, T> {
        self.transport
            .handle_operation(crate::operation::write_lba(start_sector, write))
            .await
            .map(|t| t.into())
    }

    /// Write a specific area while in maskrom mode; typically 0x471 or 0x472 data as retrieved from a
    /// rockchip boot file
    pub async fn write_maskrom_area(&mut self, area: u16, data: &[u8]) -> DeviceResult<(), T> {
        self.transport
            .handle_operation(crate::operation::write_area(area, data))
            .await
    }

    /// Reset the device
    pub async fn reset_device(&mut self, opcode: ResetOpcode) -> DeviceResult<(), T> {
        self.transport
            .handle_operation(crate::operation::reset_device(opcode))
            .await
    }
}

/// IO object which implements [Read], [Write] and [Seek]
pub struct DeviceIO<D, T> {
    device: D,
    transport: PhantomData<T>,
    size: u64,
    // Read/Write offset in bytes
    offset: u64,
    buffer: [u8; 512],
    // Whether or not the buffer is dirty
    state: BufferState,
}

impl<D, T> DeviceIO<D, T>
where
    D: BorrowMut<Device<T>>,
    T: Transport,
{
    const MAXIO_SIZE: u64 = 128 * SECTOR_SIZE;
    /// Create a new IO object around a given transport
    pub fn new(mut device: D) -> DeviceResult<Self, T> {
        let info = device.borrow_mut().flash_info()?;
        Ok(Self {
            device,
            transport: PhantomData,
            size: info.size(),
            offset: 0,
            buffer: [0u8; 512],
            state: BufferState::Invalid,
        })
    }

    /// Get a reference to the inner transport
    pub fn inner(&mut self) -> &mut Device<T> {
        self.device.borrow_mut()
    }

    /// Convert into the inner transport
    pub fn into_inner(self) -> D {
        self.device
    }

    /// Size of the flash in bytes
    pub fn size(&self) -> u64 {
        self.size
    }

    fn current_sector(&self) -> u64 {
        self.offset / SECTOR_SIZE
    }

    // Want to start an i/o operation with a given maximum length
    fn pre_io(&mut self, len: u64) -> std::result::Result<IOOperation, std::io::Error> {
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
                self.device
                    .borrow_mut()
                    .read_lba(sector, &mut self.buffer)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
                self.state = BufferState::Valid;
            }
            Ok(IOOperation::Buffered {
                offset: sector_offset as usize,
                len: len.min(sector_remaining) as usize,
            })
        }
    }

    fn post_io(&mut self, len: u64) -> std::result::Result<usize, std::io::Error> {
        // Offset inside the current sector
        let sector_offset = self.offset % SECTOR_SIZE;
        // bytes left from current position to end of current sector
        let sector_remaining = SECTOR_SIZE - sector_offset;

        // If going over the sector edge flush the current buffer and invalidate it
        if len >= sector_remaining {
            self.flush_buffer()?;
            self.state = BufferState::Invalid;
        }
        self.offset += len;
        Ok(len as usize)
    }

    fn flush_buffer(&mut self) -> std::io::Result<()> {
        if self.state == BufferState::Dirty {
            let sector = self.current_sector() as u32;
            self.device
                .borrow_mut()
                .write_lba(sector, &self.buffer)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            self.state = BufferState::Valid;
        }
        Ok(())
    }

    fn do_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let sector = self.current_sector() as u32;
        self.device
            .borrow_mut()
            .read_lba(sector, buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        Ok(buf.len())
    }

    fn do_write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let sector = self.current_sector() as u32;
        self.device
            .borrow_mut()
            .write_lba(sector, buf)
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

impl<D, T> Write for DeviceIO<D, T>
where
    D: BorrowMut<Device<T>>,
    T: Transport,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let r = match self.pre_io(buf.len() as u64)? {
            IOOperation::Direct { len } => self.do_write(&buf[..len])?,
            IOOperation::Buffered { offset, len } => {
                self.buffer[offset..offset + len].copy_from_slice(&buf[0..len]);
                self.state = BufferState::Dirty;
                len
            }
            IOOperation::Eof => {
                return Err(std::io::Error::other("Trying to write past end of area"));
            }
        };
        self.post_io(r as u64)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_buffer()
    }
}

impl<D, T> Read for DeviceIO<D, T>
where
    D: BorrowMut<Device<T>>,
    T: Transport,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let r = match self.pre_io(buf.len() as u64)? {
            IOOperation::Direct { len } => self.do_read(&mut buf[..len])?,
            IOOperation::Buffered { offset, len } => {
                buf[0..len].copy_from_slice(&self.buffer[offset..offset + len]);
                len
            }
            IOOperation::Eof => 0,
        };
        self.post_io(r as u64)
    }
}

impl<D, T> Seek for DeviceIO<D, T>
where
    D: BorrowMut<D>,
{
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.offset = match pos {
            SeekFrom::Start(offset) => self.size.min(offset),
            SeekFrom::End(offset) => {
                if offset > 0 {
                    self.size
                } else {
                    let offset = offset.unsigned_abs();
                    self.size.saturating_sub(offset)
                }
            }
            SeekFrom::Current(offset) => {
                if offset > 0 {
                    let offset = offset as u64;
                    self.offset.saturating_add(offset).min(self.size)
                } else {
                    let offset = offset.unsigned_abs();
                    self.offset.saturating_sub(offset)
                }
            }
        };
        Ok(self.offset)
    }
}
