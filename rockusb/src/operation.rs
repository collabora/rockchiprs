use std::marker::PhantomData;

use crate::protocol::{
    self, ChipInfo, CommandBlock, CommandStatus, CommandStatusParseError, Direction, FlashId,
    FlashInfo,
};
use thiserror::Error;

/// Errors for usb operations
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum UsbOperationError {
    #[error("Tag mismatch between command and status")]
    TagMismatch,
    #[error("Incorrect status Signature receveived: {0:?}")]
    InvalidStatusSignature([u8; 4]),
    #[error("Invalid status status: {0}")]
    InvalidStatusStatus(u8),
    #[error("Invalid status data length")]
    InvalidStatusLength,
    #[error("Failed to parse reply")]
    ReplyParseFailure,
    #[error("Device indicated operation failed")]
    FailedStatus,
}

impl From<CommandStatusParseError> for UsbOperationError {
    fn from(e: CommandStatusParseError) -> Self {
        match e {
            CommandStatusParseError::InvalidSignature(s) => {
                UsbOperationError::InvalidStatusSignature(s)
            }
            CommandStatusParseError::InvalidLength(_) => UsbOperationError::InvalidStatusLength,
            CommandStatusParseError::InvalidStatus(s) => UsbOperationError::InvalidStatusStatus(s),
        }
    }
}

/// Step to take by the transport implementation
#[derive(Debug, Eq, PartialEq)]
pub enum UsbStep<'a, T> {
    /// Write USB data using a control transfer
    WriteControl {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &'a [u8],
    },
    /// Write USB data using a bulk transfer
    WriteBulk { data: &'a [u8] },
    /// Read USB data using a bulk transfer
    ReadBulk { data: &'a mut [u8] },
    /// Operation is finished with a given result or failure
    Finished(Result<T, UsbOperationError>),
}

/// steps to take to finish an operation
pub trait OperationSteps<T> {
    /// Next step to execute by a transport
    fn step(&mut self) -> UsbStep<T>;
}

enum MaskRomSteps {
    Writing(crc::Digest<'static, u16>),
    Dummy,
    Done,
}

/// Operations that can be executed when the SoC is in MaskRom mode
pub struct MaskRomOperation<'a> {
    written: usize,
    block: [u8; 4096],
    data: &'a [u8],
    area: u16,
    steps: MaskRomSteps,
}

const CRC: crc::Crc<u16> = crc::Crc::<u16>::new(&crc::CRC_16_IBM_3740);
impl<'a> MaskRomOperation<'a> {
    fn new(area: u16, data: &'a [u8]) -> Self {
        Self {
            written: 0,
            block: [0; 4096],
            data,
            area,
            steps: MaskRomSteps::Writing(CRC.digest()),
        }
    }
}

impl OperationSteps<()> for MaskRomOperation<'_> {
    fn step(&mut self) -> UsbStep<()> {
        let mut current = MaskRomSteps::Done;
        std::mem::swap(&mut self.steps, &mut current);
        match current {
            MaskRomSteps::Writing(mut crc) => {
                let chunksize = 4096.min(self.data.len() - self.written);
                self.block[..chunksize]
                    .copy_from_slice(&self.data[self.written..self.written + chunksize]);
                self.written += chunksize;
                let chunk = match chunksize {
                    4096 => {
                        crc.update(&self.block);
                        self.steps = MaskRomSteps::Writing(crc);
                        &self.block[..]
                    }
                    4095 => {
                        // Add extra 0 to avoid splitting crc over two blocks
                        self.block[4095] = 0;
                        crc.update(&self.block);
                        self.steps = MaskRomSteps::Writing(crc);
                        &self.block[..]
                    }
                    mut end => {
                        crc.update(&self.block[0..end]);
                        let crc = crc.finalize();
                        self.block[end] = (crc >> 8) as u8;
                        self.block[end + 1] = (crc & 0xff) as u8;
                        end += 2;
                        if end == 4096 {
                            self.steps = MaskRomSteps::Dummy;
                        } else {
                            self.steps = MaskRomSteps::Done;
                        }

                        &self.block[0..end]
                    }
                };

                UsbStep::WriteControl {
                    request_type: 0x40,
                    request: 0xc,
                    value: 0,
                    index: self.area,
                    data: chunk,
                }
            }
            MaskRomSteps::Dummy => {
                self.steps = MaskRomSteps::Done;
                self.block[0] = 0;
                UsbStep::WriteControl {
                    request_type: 0x40,
                    request: 0xc,
                    value: 0,
                    index: self.area,
                    data: &self.block[0..1],
                }
            }
            MaskRomSteps::Done => UsbStep::Finished(Ok(())),
        }
    }
}

/// Write a specific area; typically 0x471 or 0x472 data as retrieved from a rockchip boot file
pub fn write_area(area: u16, data: &[u8]) -> MaskRomOperation {
    MaskRomOperation::new(area, data)
}

trait FromOperation {
    fn from_operation(io: &[u8], status: &CommandStatus) -> Result<Self, UsbOperationError>
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

/// Operation to execute using the "full" USB protocol
pub struct UsbOperation<'a, T> {
    command: CommandBlock,
    command_bytes: [u8; 31],
    data: IOBytes<'a>,
    next: Operation,
    _result: PhantomData<T>,
}

impl<'a, T> UsbOperation<'a, T> {
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
            IOBytes::Write(data) => data,
        }
    }
}

impl<T> OperationSteps<T> for UsbOperation<'_, T>
where
    T: FromOperation,
    T: std::fmt::Debug,
{
    fn step(&mut self) -> UsbStep<T> {
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
                    .map_err(UsbOperationError::from)
                    .and_then(|csw| {
                        if csw.status == protocol::Status::FAILED {
                            Err(UsbOperationError::FailedStatus)
                        } else if csw.tag == self.command.tag() {
                            let transfer = self.command.transfer_length() as usize;
                            T::from_operation(&self.io_data()[..transfer], &csw)
                        } else {
                            Err(UsbOperationError::TagMismatch)
                        }
                    });
                UsbStep::Finished(r)
            }
        }
    }
}

impl FromOperation for ChipInfo {
    fn from_operation(io: &[u8], _status: &CommandStatus) -> Result<Self, UsbOperationError>
    where
        Self: Sized,
    {
        let data = io
            .try_into()
            .map_err(|_e| UsbOperationError::ReplyParseFailure)?;
        Ok(ChipInfo::from_bytes(data))
    }
}

/// Create operation to retrieve SoC Chip information
pub fn chip_info() -> UsbOperation<'static, ChipInfo> {
    UsbOperation::new(CommandBlock::chip_info())
}

impl FromOperation for FlashId {
    fn from_operation(io: &[u8], _status: &CommandStatus) -> Result<Self, UsbOperationError>
    where
        Self: Sized,
    {
        let data = io
            .try_into()
            .map_err(|_e| UsbOperationError::ReplyParseFailure)?;
        Ok(FlashId::from_bytes(data))
    }
}

/// Create operation to retrieve SoC flash identifier
pub fn flash_id() -> UsbOperation<'static, FlashId> {
    UsbOperation::new(CommandBlock::flash_id())
}

impl FromOperation for FlashInfo {
    fn from_operation(io: &[u8], _status: &CommandStatus) -> Result<Self, UsbOperationError>
    where
        Self: Sized,
    {
        let data = io
            .try_into()
            .map_err(|_e| UsbOperationError::ReplyParseFailure)?;
        Ok(FlashInfo::from_bytes(data))
    }
}

/// Create operation to retrieve SoC flash information
pub fn flash_info() -> UsbOperation<'static, FlashInfo> {
    UsbOperation::new(CommandBlock::flash_info())
}

/// Bytes transferred
#[derive(Debug, Clone, Copy)]
pub struct Transferred(u32);
impl FromOperation for Transferred {
    fn from_operation(io: &[u8], status: &CommandStatus) -> Result<Self, UsbOperationError>
    where
        Self: Sized,
    {
        let totransfer = io.len() as u32;
        if status.residue > totransfer {
            Err(UsbOperationError::ReplyParseFailure)
        } else {
            Ok(Transferred(totransfer - status.residue))
        }
    }
}

impl From<Transferred> for u32 {
    fn from(t: Transferred) -> Self {
        t.0
    }
}

/// Create operation to read an lba from the flash
///
/// start_sector with [protocol::SECTOR_SIZE] sectors. the data to be read must be a multiple of
/// [protocol::SECTOR_SIZE] bytes
pub fn read_lba(start_sector: u32, read: &mut [u8]) -> UsbOperation<'_, Transferred> {
    assert_eq!(read.len() % 512, 0, "Not a multiple of 512: {}", read.len());
    UsbOperation::new_read(
        CommandBlock::read_lba(start_sector, (read.len() / 512) as u16),
        read,
    )
}

/// Create operation to read an lba from the flash
///
/// start_sector with [protocol::SECTOR_SIZE] sectors. the data to be written must be a multiple of
/// [protocol::SECTOR_SIZE] bytes
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
                [data[4], data[5], data[6], data[7]]
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
