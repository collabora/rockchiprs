#[cfg(feature = "async")]
use std::task::Poll;
use std::{
    borrow::BorrowMut,
    io::{Read, Seek, SeekFrom, Write},
    marker::PhantomData,
};

use crate::{
    operation::OperationSteps,
    protocol::{Capability, ChipInfo, FlashId, FlashInfo, ResetOpcode, SECTOR_SIZE},
};

#[cfg(feature = "async")]
use futures::{AsyncRead, AsyncSeek, AsyncWrite, future::BoxFuture, ready};
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
            DeviceIO(async = "DeviceIOAsync"),
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

    #[maybe_async_cfg::only_if(sync)]
    /// Create an IO object which implements [Read], [Write] and
    /// [Seek]
    pub async fn io(&mut self) -> DeviceResult<DeviceIO<&mut Self, T>, T> {
        DeviceIO::new(self).await
    }

    /// Convert into an IO object which implements [Read], [Write] and
    /// [Seek]
    pub async fn into_io(self) -> DeviceResult<DeviceIO<Self, T>, T> {
        DeviceIO::new(self).await
    }
}

const MAXIO_SIZE: u64 = 128 * crate::protocol::SECTOR_SIZE;

#[maybe_async_cfg::maybe(sync(keep_self), async(feature = "async"))]
struct DeviceIOInner<D, T> {
    device: D,
    transport: PhantomData<T>,
    // Read/Write offset in bytes
    offset: u64,
    size: u64,
    buffer: Box<[u8; 512]>,
    // Whether or not the buffer is dirty
    state: BufferState,
}

/// IO object which implements [Read], [Write] and [Seek]
pub struct DeviceIO<D, T> {
    inner: DeviceIOInner<D, T>,
}

impl<D, T> DeviceIO<D, T>
where
    D: BorrowMut<Device<T>>,
    T: Transport,
{
    /// Create a new IO object around a given transport
    pub fn new(mut device: D) -> DeviceResult<Self, T> {
        let info = device.borrow_mut().flash_info()?;
        let size = info.size();
        Ok(Self {
            inner: DeviceIOInner {
                device,
                transport: PhantomData,
                offset: 0,
                size,
                buffer: Box::new([0u8; 512]),
                state: BufferState::Invalid,
            },
        })
    }

    /// Get a reference to the inner transport
    pub fn inner(&mut self) -> &mut Device<T> {
        self.inner.device.borrow_mut()
    }

    /// Convert into the inner transport
    pub fn into_inner(self) -> D {
        self.inner.device
    }

    pub fn size(&self) -> u64 {
        self.inner.size
    }
}

#[maybe_async_cfg::maybe(
    sync(keep_self),
    async(
        feature = "async",
        idents(
            Device(async = "DeviceAsync"),
            DeviceResult(async = "DeviceResultAsync"),
            Transport(async = "TransportAsync")
        )
    )
)]
impl<D, T> DeviceIOInner<D, T>
where
    D: BorrowMut<Device<T>>,
    T: Transport,
{
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
                len: io_len.min(MAXIO_SIZE) as usize,
            })
        } else {
            if self.state == BufferState::Invalid {
                let sector = self.current_sector() as u32;
                self.device
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
            self.device
                .borrow_mut()
                .write_lba(sector, self.buffer.as_mut())
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            self.state = BufferState::Valid;
        }
        Ok(())
    }

    async fn read_lba(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let sector = self.current_sector() as u32;
        self.device
            .borrow_mut()
            .read_lba(sector, buf)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        Ok(buf.len())
    }

    async fn write_lba(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let sector = self.current_sector() as u32;
        self.device
            .borrow_mut()
            .write_lba(sector, buf)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        Ok(buf.len())
    }

    fn do_seek(&mut self, pos: SeekFrom) -> u64 {
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
        self.offset
    }

    async fn do_write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let r = match self.pre_io(buf.len() as u64).await? {
            IOOperation::Direct { len } => self.write_lba(&buf[..len]).await?,
            IOOperation::Buffered { offset, len } => {
                self.buffer[offset..offset + len].copy_from_slice(&buf[0..len]);
                self.state = BufferState::Dirty;
                len
            }
            IOOperation::Eof => {
                return Err(std::io::Error::other("Trying to write past end of area"));
            }
        };
        self.post_io(r as u64).await
    }

    async fn do_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let r = match self.pre_io(buf.len() as u64).await? {
            IOOperation::Direct { len } => self.read_lba(&mut buf[..len]).await?,
            IOOperation::Buffered { offset, len } => {
                buf[0..len].copy_from_slice(&self.buffer[offset..offset + len]);
                len
            }
            IOOperation::Eof => 0,
        };
        self.post_io(r as u64).await
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
        self.inner.do_write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush_buffer()
    }
}

impl<D, T> Read for DeviceIO<D, T>
where
    D: BorrowMut<Device<T>>,
    T: Transport,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.do_read(buf)
    }
}

impl<D, T> Seek for DeviceIO<D, T>
where
    D: BorrowMut<Device<T>>,
    T: Transport,
{
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        Ok(self.inner.do_seek(pos))
    }
}

#[cfg(feature = "async")]
type ReadResult = std::io::Result<(Vec<u8>, usize)>;
#[cfg(feature = "async")]
enum IoState<D, T> {
    Idle(Option<DeviceIOInnerAsync<D, T>>),
    Read(BoxFuture<'static, (DeviceIOInnerAsync<D, T>, ReadResult)>),
    Write(BoxFuture<'static, (DeviceIOInnerAsync<D, T>, std::io::Result<usize>)>),
    Flush(BoxFuture<'static, (DeviceIOInnerAsync<D, T>, std::io::Result<()>)>),
}

/// IO object which implements [AsyncRead], [AsyncWrite] and [AsyncSeek]
#[cfg(feature = "async")]
pub struct DeviceIOAsync<D, T> {
    // io execution state
    io_state: IoState<D, T>,
    size: u64,
}

#[cfg(feature = "async")]
impl<T> DeviceIOAsync<DeviceAsync<T>, T>
where
    T: TransportAsync,
{
    /// Create a new IO object around a given transport
    pub async fn new(mut device: DeviceAsync<T>) -> DeviceResultAsync<Self, T> {
        let info = device.borrow_mut().flash_info().await?;
        let size = info.size();
        let inner = DeviceIOInnerAsync {
            device,
            transport: PhantomData,
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

    pub fn size(&self) -> u64 {
        self.size
    }
}

#[cfg(feature = "async")]
impl<T> AsyncWrite for DeviceIOAsync<DeviceAsync<T>, T>
where
    T: TransportAsync + Unpin + Send + 'static,
{
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
                    let buf = Vec::from(&buf[0..buf.len().min(MAXIO_SIZE as usize)]);
                    me.io_state = IoState::Write(Box::pin(async move {
                        let r = inner.do_write(&buf).await;
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
                    )));
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
                    )));
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

#[cfg(feature = "async")]
impl<T> AsyncRead for DeviceIOAsync<DeviceAsync<T>, T>
where
    T: TransportAsync + Unpin + Send + 'static,
{
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
                let r = inner.do_read(&mut buf).await.map(|r| (buf, r));
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

#[cfg(feature = "async")]
impl<T> AsyncSeek for DeviceIOAsync<DeviceAsync<T>, T>
where
    T: TransportAsync + Unpin,
{
    fn poll_seek(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        pos: SeekFrom,
    ) -> std::task::Poll<futures::io::Result<u64>> {
        let me = self.get_mut();
        match me.io_state {
            IoState::Idle(Some(ref mut inner)) => Poll::Ready(Ok(inner.do_seek(pos))),
            _ => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Invalid transport state",
            ))),
        }
    }
}
