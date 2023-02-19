use std::{
    io::{Read, Seek, SeekFrom, Write},
    time::Duration,
};

use crate::{
    protocol::{ChipInfo, FlashId, FlashInfo},
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
    offset: u32,
    written: usize,
    outstanding_write: usize,
    write_buffer: [u8; 512],
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
            offset: 0,
            written: 0,
            outstanding_write: 0,
            write_buffer: [0u8; 512],
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

impl Write for LibUsbTransport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.outstanding_write > 0 {
            let left = 512 - self.outstanding_write;
            let copy = left.min(buf.len());
            let end = self.outstanding_write + copy;

            self.write_buffer[self.outstanding_write..end].copy_from_slice(&buf[0..copy]);
            self.outstanding_write += copy;
            assert!(self.outstanding_write <= 512);

            if self.outstanding_write == 512 {
                let towrite = self.write_buffer;
                self.write_lba(self.offset, &towrite).unwrap();
                self.offset += 1;
                self.outstanding_write = 0;
            }
            return Ok(copy);
        }

        let old = self.offset;
        const CHUNK: usize = 128 * 512;
        let written = if buf.len() > CHUNK {
            self.write_lba(self.offset, &buf[0..CHUNK]).unwrap();
            self.offset += (CHUNK / 512) as u32;
            CHUNK
        } else if buf.len() >= 512 {
            let towrite = buf.len() / 512;
            let towrite = towrite * 512;
            self.write_lba(self.offset, &buf[0..towrite]).unwrap();
            self.offset += (towrite / 512) as u32;
            towrite
        } else {
            self.write_buffer[0..buf.len()].copy_from_slice(buf);
            self.outstanding_write = buf.len();

            buf.len()
        };

        // Report every 128 MB; mind in sectors
        const REPORTING: usize = 128 * 1024 * 2;

        // HACK show how far in the write we are
        if old as usize / REPORTING != self.offset as usize / REPORTING {
            println!(
                "At {} MB (written {})",
                self.offset / 2048,
                self.written / (1024 * 1024)
            );
        }
        self.written += written;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.outstanding_write > 0 {
            eprintln!(
                "Write flush for small write flush : {}",
                self.outstanding_write
            );
            let mut read = [0u8; 512];
            self.read_lba(self.offset, &mut read).unwrap();
            read[0..self.outstanding_write].copy_from_slice(&self.write_buffer);
            self.write_lba(self.offset, &read).unwrap();

            let mut check = [0u8; 512];
            self.read_lba(self.offset, &mut check).unwrap();
            assert_eq!(check, read);

            self.outstanding_write = 0;
            self.offset += 1;
        }

        Ok(())
    }
}

impl Read for LibUsbTransport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let toread = buf.len().min(128 * 512);
        assert!(toread % 512 == 0);
        self.read_lba(self.offset, &mut buf[0..toread]).unwrap();
        self.offset += toread as u32 / 512;
        Ok(toread)
    }
}

impl Seek for LibUsbTransport {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.flush()?;
        match pos {
            SeekFrom::Start(offset) => {
                assert!(offset % 512 == 0, "Not 512 multiple: {}", offset);
                self.offset = (offset / 512) as u32;
            }
            SeekFrom::Current(offset) => {
                assert!(offset % 512 == 0, "Not 512 multiple: {}", offset);
                self.offset += (offset / 512) as u32;
            }
            SeekFrom::End(_) => todo!(),
        }
        Ok(self.offset as u64 * 512)
    }
}
