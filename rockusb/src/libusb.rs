use std::{
    borrow::BorrowMut,
    io::{Read, Seek, SeekFrom, Write},
    time::Duration,
};

use crate::{
    protocol::{ChipInfo, FlashId, FlashInfo, SECTOR_SIZE},
    OperationSteps,
};
use rusb::{DeviceHandle, GlobalContext};
use thiserror::Error;

#[derive(Debug, Clone, Eq, PartialEq, Error)]
#[error("Device is not available: {device:?} {error}")]
pub struct DeviceUnavalable {
    pub device: rusb::Device<GlobalContext>,
    #[source]
    pub error: rusb::Error,
}

#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum LibUsbError {
    #[error("Usb error: {0}")]
    UsbError(#[from] rusb::Error),
    #[error("Operation error: {0}")]
    OperationError(#[from] crate::RockUsbOperationError),
}
type Result<T> = std::result::Result<T, LibUsbError>;

pub struct Devices {
    devices: rusb::DeviceList<GlobalContext>,
}

impl Devices {
    pub fn new() -> Result<Self> {
        let devices = rusb::DeviceList::new()?;
        Ok(Self { devices })
    }

    pub fn iter(&self) -> DevicesIter {
        let iter = self.devices.iter();
        DevicesIter { iter }
    }
}

pub struct DevicesIter<'a> {
    iter: rusb::Devices<'a, GlobalContext>,
}

impl Iterator for DevicesIter<'_> {
    type Item = std::result::Result<LibUsbTransport, DeviceUnavalable>;

    fn next(&mut self) -> Option<Self::Item> {
        for device in self.iter.by_ref() {
            let desc = match device.device_descriptor() {
                Ok(desc) => desc,
                _ => continue,
            };
            if desc.vendor_id() != 0x2207 {
                continue;
            }
            let handle = match device.open() {
                Ok(handle) => handle,
                Err(error) => return Some(Err(DeviceUnavalable { device, error })),
            };

            return Some(LibUsbTransport::from_usb_device(handle));
        }
        None
    }
}

pub struct LibUsbTransport {
    handle: DeviceHandle<rusb::GlobalContext>,
    ep_in: u8,
    ep_out: u8,
}

impl LibUsbTransport {
    fn new(
        mut handle: DeviceHandle<rusb::GlobalContext>,
        interface: u8,
        ep_in: u8,
        ep_out: u8,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        handle
            .claim_interface(interface)
            .map_err(|error| DeviceUnavalable {
                device: handle.device(),
                error,
            })?;
        Ok(Self {
            handle,
            ep_in,
            ep_out,
        })
    }

    pub fn from_usb_device(
        handle: rusb::DeviceHandle<GlobalContext>,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        let device = handle.device();
        let desc = device
            .device_descriptor()
            .map_err(|error| DeviceUnavalable {
                device: device.clone(),
                error,
            })?;
        for c in 0..desc.num_configurations() {
            let config = device
                .config_descriptor(c)
                .map_err(|error| DeviceUnavalable {
                    device: device.clone(),
                    error,
                })?;
            for i in config.interfaces() {
                for i_desc in i.descriptors() {
                    let output = i_desc.endpoint_descriptors().find(|e| {
                        e.direction() == rusb::Direction::Out
                            && e.transfer_type() == rusb::TransferType::Bulk
                    });
                    let input = i_desc.endpoint_descriptors().find(|e| {
                        e.direction() == rusb::Direction::In
                            && e.transfer_type() == rusb::TransferType::Bulk
                    });

                    if let (Some(input), Some(output)) = (input, output) {
                        return LibUsbTransport::new(
                            handle,
                            i_desc.setting_number(),
                            input.address(),
                            output.address(),
                        );
                    }
                }
            }
        }
        Err(DeviceUnavalable {
            device,
            error: rusb::Error::NotFound,
        })
    }

    pub fn io(&mut self) -> Result<LibUsbTransportIO<&mut Self>> {
        LibUsbTransportIO::new(self)
    }

    pub fn into_io(self) -> Result<LibUsbTransportIO<Self>> {
        LibUsbTransportIO::new(self)
    }

    pub fn handle(&mut self) -> &mut DeviceHandle<GlobalContext> {
        &mut self.handle
    }

    pub fn bus_number(&self) -> u8 {
        self.handle.device().bus_number()
    }

    pub fn address(&self) -> u8 {
        self.handle.device().address()
    }

    fn handle_operation<O, T>(&mut self, mut operation: O) -> Result<T>
    where
        O: OperationSteps<T>,
    {
        loop {
            let step = operation.step();
            match step {
                crate::UsbStep::WriteBulk { data } => {
                    let _written =
                        self.handle
                            .write_bulk(self.ep_out, data, Duration::from_secs(5))?;
                }
                crate::UsbStep::ReadBulk { data } => {
                    let _read = self
                        .handle
                        .read_bulk(self.ep_in, data, Duration::from_secs(5))?;
                }
                crate::UsbStep::Finished(r) => break r.map_err(|e| e.into()),
                crate::UsbStep::WriteControl {
                    request_type,
                    request,
                    value,
                    index,
                    data,
                } => {
                    self.handle.write_control(
                        request_type,
                        request,
                        value,
                        index,
                        data,
                        Duration::from_secs(5),
                    )?;
                }
            }
        }
    }

    pub fn flash_id(&mut self) -> Result<FlashId> {
        self.handle_operation(crate::flash_id())
    }

    pub fn flash_info(&mut self) -> Result<FlashInfo> {
        self.handle_operation(crate::flash_info())
    }

    pub fn chip_info(&mut self) -> Result<ChipInfo> {
        self.handle_operation(crate::chip_info())
    }

    pub fn read_lba(&mut self, start_sector: u32, read: &mut [u8]) -> Result<u32> {
        self.handle_operation(crate::read_lba(start_sector, read))
            .map(|t| t.into())
    }

    pub fn write_lba(&mut self, start_sector: u32, write: &[u8]) -> Result<u32> {
        self.handle_operation(crate::write_lba(start_sector, write))
            .map(|t| t.into())
    }

    pub fn write_maskrom_area(&mut self, area: u16, data: &[u8]) -> Result<()> {
        self.handle_operation(crate::write_area(area, data))
    }
}

pub struct LibUsbTransportIO<T> {
    transport: T,
    size: u64,
    // Read/Write offset in bytes
    offset: u64,
    buffer: [u8; 512],
    // Whether or not the buffer is dirty
    state: BufferState,
}

impl<T> LibUsbTransportIO<T>
where
    T: BorrowMut<LibUsbTransport>,
{
    const MAXIO_SIZE: u64 = 128 * crate::protocol::SECTOR_SIZE as u64;
    pub fn new(mut transport: T) -> Result<Self> {
        let info = transport.borrow_mut().flash_info()?;
        Ok(Self {
            transport,
            size: info.size(),
            offset: 0,
            buffer: [0u8; 512],
            state: BufferState::Invalid,
        })
    }

    pub fn inner(&mut self) -> &mut LibUsbTransport {
        self.transport.borrow_mut()
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    fn current_sector(&self) -> u64 {
        self.offset / SECTOR_SIZE
    }

    // Want to start an i/o operation with a given maximum length
    fn pre_io(&mut self, len: u64) -> std::result::Result<IOOperation, std::io::Error> {
        // Offset inside the current sector
        let sector_offset = self.offset % SECTOR_SIZE;
        // bytes left from current position to end of current sector
        let sector_remaining = SECTOR_SIZE - sector_offset;

        // If the I/O operation is starting at a sector edge and encompasses at least one sector
        // then direct I/O can be done
        if sector_offset == 0 && len >= SECTOR_SIZE {
            let io_len = len / SECTOR_SIZE * SECTOR_SIZE;
            Ok(IOOperation::Direct {
                len: io_len.min(Self::MAXIO_SIZE) as usize,
            })
        } else {
            if self.state == BufferState::Invalid {
                let sector = self.current_sector() as u32;
                self.transport
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
            self.transport
                .borrow_mut()
                .write_lba(sector, &self.buffer)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            self.state = BufferState::Valid;
        }
        Ok(())
    }

    fn do_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let sector = self.current_sector() as u32;
        self.transport
            .borrow_mut()
            .read_lba(sector, buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        Ok(buf.len())
    }

    fn do_write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let sector = self.current_sector() as u32;
        self.transport
            .borrow_mut()
            .write_lba(sector, buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        Ok(buf.len())
    }
}

enum IOOperation {
    Direct { len: usize },
    Buffered { offset: usize, len: usize },
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

impl<T> Write for LibUsbTransportIO<T>
where
    T: BorrowMut<LibUsbTransport>,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let r = match self.pre_io(buf.len() as u64)? {
            IOOperation::Direct { len } => self.do_write(&buf[..len])?,
            IOOperation::Buffered { offset, len } => {
                self.buffer[offset..offset + len].copy_from_slice(&buf[0..len]);
                self.state = BufferState::Dirty;
                len
            }
        };
        self.post_io(r as u64)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_buffer()
    }
}

impl<T> Read for LibUsbTransportIO<T>
where
    T: BorrowMut<LibUsbTransport>,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let r = match self.pre_io(buf.len() as u64)? {
            IOOperation::Direct { len } => self.do_read(&mut buf[..len])?,
            IOOperation::Buffered { offset, len } => {
                buf[0..len].copy_from_slice(&self.buffer[offset..offset + len]);
                len
            }
        };
        self.post_io(r as u64)
    }
}

impl<T> Seek for LibUsbTransportIO<T>
where
    T: BorrowMut<LibUsbTransport>,
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
