use std::marker::PhantomData;

use protocol::{ChipInfo, CommandBlock, CommandStatus, Direction, FlashInfo};
use thiserror::Error;

pub mod bootfile;
pub mod protocol;

#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum RockUsbOperationError {
    #[error("Setup good error types")]
    TODO,
}

pub enum MaskRomStep<'a> {
    WriteControl {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &'a [u8],
    },
    FillData {
        data: &'a mut [u8],
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum UsbStep<'a, T> {
    WriteBulk { data: &'a [u8] },
    ReadBulk { data: &'a mut [u8] },
    Finished(Result<T, RockUsbOperationError>),
}

pub trait FromOperation {
    fn from_operation(io: &[u8], status: &CommandStatus) -> Result<Self, RockUsbOperationError>
    where
        Self: Sized;
}

enum Operation {
    CommandBlock,
    IO,
    CommandStatus,
    Finish,
}

enum IOBytes<'a> {
    // Biggest transfer that's not a data read/write
    Inband([u8; 16]),
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

pub struct UsbOperation<'a, T> {
    command: CommandBlock,
    command_bytes: [u8; 31],
    data: IOBytes<'a>,
    next: Operation,
    _result: PhantomData<T>,
}

impl<'a, T> UsbOperation<'a, T>
where
    T: FromOperation,
    T: std::fmt::Debug,
{
    fn new(command: CommandBlock) -> Self {
        Self {
            command,
            command_bytes: [0u8; protocol::COMMAND_BLOCK_BYTES],
            data: IOBytes::Inband([0u8; 16]),
            next: Operation::CommandBlock,
            _result: PhantomData,
        }
    }

    fn new_write(command: CommandBlock, data: &'a [u8]) -> Self {
        Self {
            command,
            command_bytes: [0u8; protocol::COMMAND_BLOCK_BYTES],
            data: IOBytes::Write(data),
            next: Operation::CommandBlock,
            _result: PhantomData,
        }
    }

    fn new_read(command: CommandBlock, data: &'a mut [u8]) -> Self {
        Self {
            command,
            command_bytes: [0u8; protocol::COMMAND_BLOCK_BYTES],
            data: IOBytes::Read(data),
            next: Operation::CommandBlock,
            _result: PhantomData,
        }
    }

    fn io_data_mut(&mut self) -> &mut [u8] {
        match &mut self.data {
            IOBytes::Inband(ref mut data) => data,
            IOBytes::Read(ref mut data) => data,
            IOBytes::Write(_) => unreachable!(),
        }
    }

    fn io_data(&mut self) -> &[u8] {
        match self.data {
            IOBytes::Inband(ref data) => data,
            IOBytes::Read(ref data) => data,
            IOBytes::Write(ref data) => data,
        }
    }

    pub fn step(&mut self) -> UsbStep<T> {
        let mut next = Operation::CommandBlock;
        std::mem::swap(&mut self.next, &mut next);
        match next {
            Operation::CommandBlock => {
                let len = self.command.to_bytes(&mut self.command_bytes);
                self.next = Operation::IO;
                UsbStep::WriteBulk {
                    data: &self.command_bytes[..len],
                }
            }
            Operation::IO => {
                self.next = Operation::CommandStatus;
                let len = self.command.transfer_length() as usize;
                match self.command.direction() {
                    Direction::Out => UsbStep::WriteBulk {
                        data: &self.io_data()[..len],
                    },
                    Direction::In => UsbStep::ReadBulk {
                        data: &mut self.io_data_mut()[..len],
                    },
                }
            }
            Operation::CommandStatus => {
                self.next = Operation::Finish;
                UsbStep::ReadBulk {
                    data: &mut self.command_bytes[..protocol::COMMAND_STATUS_BYTES],
                }
            }
            Operation::Finish => {
                let r = CommandStatus::from_bytes(&self.command_bytes)
                    .map_err(|_e| RockUsbOperationError::TODO)
                    .and_then(|csw| {
                        let transfer = self.command.transfer_length() as usize;
                        T::from_operation(&self.io_data()[..transfer], &csw)
                    });
                UsbStep::Finished(r)
            }
        }
    }
}

impl FromOperation for ChipInfo {
    fn from_operation(io: &[u8], _status: &CommandStatus) -> Result<Self, RockUsbOperationError>
    where
        Self: Sized,
    {
        let data = io.try_into().map_err(|_e| RockUsbOperationError::TODO)?;
        Ok(ChipInfo::from_bytes(data))
    }
}

pub fn chip_info() -> UsbOperation<'static, ChipInfo> {
    UsbOperation::new(CommandBlock::chip_info())
}

impl FromOperation for FlashInfo {
    fn from_operation(io: &[u8], _status: &CommandStatus) -> Result<Self, RockUsbOperationError>
    where
        Self: Sized,
    {
        let data = io.try_into().map_err(|_e| RockUsbOperationError::TODO)?;
        Ok(FlashInfo::from_bytes(data))
    }
}
pub fn flash_info() -> UsbOperation<'static, FlashInfo> {
    UsbOperation::new(CommandBlock::flash_info())
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct Transferred(u32);
impl FromOperation for Transferred {
    fn from_operation(io: &[u8], status: &CommandStatus) -> Result<Self, RockUsbOperationError>
    where
        Self: Sized,
    {
        let totransfer = io.len() as u32;
        if status.residue > totransfer {
            Err(RockUsbOperationError::TODO)
        } else {
            Ok(Transferred(totransfer - status.residue))
        }
    }
}

pub fn read_lba(start_sector: u32, read: &mut [u8]) -> UsbOperation<'_, Transferred> {
    assert_eq!(read.len() % 512, 0, "Not a multiple of 512: {}", read.len());
    UsbOperation::new_read(
        CommandBlock::read_lba(start_sector, (read.len() / 512) as u16),
        read,
    )
}

pub fn write_lba(start_sector: u32, write: &[u8]) -> UsbOperation<'_, Transferred> {
    assert_eq!(
        write.len() % 512,
        0,
        "Not a multiple of 512: {}",
        write.len()
    );
    UsbOperation::new_write(
        CommandBlock::write_lba(start_sector, (write.len() / 512) as u16),
        write,
    )
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn chip_info_operation() {
        let mut o = chip_info();
        let cb = CommandBlock::chip_info();
        let mut cb_bytes = [0u8; protocol::COMMAND_BLOCK_BYTES];
        cb.to_bytes(&mut cb_bytes);
        // Write the command block
        let tag = match o.step() {
            UsbStep::WriteBulk { data } if data.len() == protocol::COMMAND_BLOCK_BYTES => {
                [data[0], data[1], data[2], data[3]]
            }
            o => panic!("Unexpected step: {:?}", o),
        };

        // Reading the chip info
        match o.step() {
            UsbStep::ReadBulk { data } if data.len() as u32 == cb.transfer_length() => {
                data.fill(0);
                /* 3588 */
                data[0] = 0x38;
                data[1] = 0x38;
                data[2] = 0x35;
                data[3] = 0x33;
            }
            o => panic!("Unexpected step: {:?}", o),
        }

        // reading status
        match o.step() {
            UsbStep::ReadBulk { data } if data.len() == protocol::COMMAND_STATUS_BYTES => {
                data.fill(0);
                /* signature */
                data[0] = b'U';
                data[1] = b'S';
                data[2] = b'B';
                data[3] = b'S';
                /* tag */
                data[4] = tag[0];
                data[5] = tag[1];
                data[6] = tag[2];
                data[7] = tag[3];
                // status good
                data[12] = 0;
            }
            o => panic!("Unexpected step: {:?}", o),
        }

        match o.step() {
            UsbStep::Finished(Ok(info)) => {
                assert_eq!(
                    info.inner(),
                    [
                        0x38u8, 0x38, 0x35, 0x33, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
                        0x0, 0x0
                    ]
                )
            }
            o => panic!("Unexpected step: {:?}", o),
        }
    }
}
