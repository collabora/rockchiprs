use bytes::{Buf, BufMut};
use num_enum::{IntoPrimitive, TryFromPrimitive};

const SECTOR_SIZE: u32 = 512;

#[repr(u8)]
#[derive(Debug, Eq, PartialEq, Clone, Copy, IntoPrimitive, TryFromPrimitive)]
pub enum Direction {
    In = 0x80,
    Out = 0x0,
}

#[non_exhaustive]
#[repr(u8)]
#[derive(Debug, Eq, PartialEq, Clone, Copy, IntoPrimitive, TryFromPrimitive)]
enum CommandCode {
    TestUnitReady = 0,
    ReadFlashId = 0x01,
    TestBadBlock = 0x03,
    ReadSector = 0x04,
    WriteSector = 0x05,
    EraseNormal = 0x06,
    EraseForce = 0x0B,
    ReadLBA = 0x14,
    WriteLBA = 0x15,
    EraseSystemDisk = 0x16,
    ReadSDram = 0x17,
    WriteSDram = 0x18,
    ExecuteSDram = 0x19,
    ReadFlashInfo = 0x1A,
    ReadChipInfo = 0x1B,
    SetResetFlag = 0x1E,
    WriteEFuse = 0x1F,
    ReadEFuse = 0x20,
    ReadSPIFlash = 0x21,
    WriteSPIFlash = 0x22,
    WriteNewEfuse = 0x23,
    ReadNewEfuse = 0x24,
    EraseLBA = 0x25,
    ReadCapability = 0xAA,
    DeviceReset = 0xFF,
}

#[derive(Debug, thiserror::Error, Clone)]
pub enum CommandStatusParseError {
    #[error("Invalid signature: {0:x?}")]
    InvalidSignature([u8; 4]),
    #[error("Invalid length: {0}")]
    InvalidLength(usize),
    #[error("Invalid status: {0}")]
    InvalidStatus(u8),
}

#[repr(u8)]
#[derive(Debug, Eq, PartialEq, Clone, Copy, IntoPrimitive, TryFromPrimitive)]
pub enum Status {
    SUCCESS = 0,
    FAILED = 1,
}

pub const COMMAND_STATUS_BYTES: usize = 13;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandStatus {
    pub tag: u32,
    pub residue: u32,
    pub status: Status,
}

impl CommandStatus {
    pub fn to_bytes(&self, bytes: &mut [u8]) -> usize {
        let mut bytes = &mut bytes[..];
        bytes.put_slice(b"USBS");
        bytes.put_u32(self.tag);
        bytes.put_u32_le(self.residue);
        bytes.put_u8(self.status.into());
        COMMAND_STATUS_BYTES
    }

    pub fn from_bytes(mut bytes: &[u8]) -> Result<CommandStatus, CommandStatusParseError> {
        if bytes.len() < COMMAND_STATUS_BYTES {
            return Err(CommandStatusParseError::InvalidLength(bytes.len()));
        }
        let mut magic = [0u8; 4];
        bytes.copy_to_slice(&mut magic);
        if &magic != b"USBS" {
            return Err(CommandStatusParseError::InvalidSignature(magic));
        }
        let tag = bytes.get_u32();
        let residue = bytes.get_u32_le();
        let status = Status::try_from(bytes.get_u8())
            .map_err(|e| CommandStatusParseError::InvalidStatus(e.number))?;
        Ok(CommandStatus {
            tag,
            residue,
            status,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChipInfo([u8; 16]);
impl ChipInfo {
    pub fn from_bytes(data: [u8; 16]) -> Self {
        ChipInfo(data)
    }

    pub fn inner(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FlashInfo([u8; 11]);
impl FlashInfo {
    pub fn from_bytes(data: [u8; 11]) -> Self {
        FlashInfo(data)
    }

    /// size in 512 byte sectors
    pub fn sectors(&self) -> u32 {
        self.0.as_slice().get_u32_le()
    }

    /// Block size in 512 bytes sectors
    pub fn block_size_sectors(&self) -> u16 {
        (&self.0[4..]).get_u16_le()
    }

    pub fn inner(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, thiserror::Error, Clone)]
pub enum CommandBlockParseError {
    #[error("Invalid Command block signature: {0:x?}")]
    InvalidSignature([u8; 4]),
    #[error("Unknown Command code : {0:x}")]
    UnknownCommandCode(u8),
    #[error("Unknown flags: {0:x}")]
    UnknownFlags(u8),
    #[error("Invalid command block length: {0}")]
    InvalidLength(usize),
}

pub const COMMAND_BLOCK_BYTES: usize = 31;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBlock {
    tag: u32,
    transfer_length: u32,
    flags: Direction,
    lun: u8,
    // Length of command data block
    cdb_length: u8,
    // Command data block fields
    cd_code: CommandCode,
    cd_address: u32,
    cd_length: u16,
}

impl CommandBlock {
    pub fn flash_info() -> CommandBlock {
        CommandBlock {
            tag: fastrand::u32(..),
            transfer_length: 11,
            flags: Direction::In,
            lun: 0,
            cdb_length: 0x6,
            cd_code: CommandCode::ReadFlashInfo,
            cd_address: 0,
            cd_length: 0x0,
        }
    }

    pub fn chip_info() -> CommandBlock {
        CommandBlock {
            tag: fastrand::u32(..),
            transfer_length: 16,
            flags: Direction::In,
            lun: 0,
            cdb_length: 0x6,
            cd_code: CommandCode::ReadChipInfo,
            cd_address: 0,
            cd_length: 0x0,
        }
    }

    pub fn read_lba(start_sector: u32, sectors: u16) -> CommandBlock {
        CommandBlock {
            tag: fastrand::u32(..),
            transfer_length: u32::from(sectors) * SECTOR_SIZE,
            flags: Direction::In,
            lun: 0,
            cdb_length: 0xa,
            cd_code: CommandCode::ReadLBA,
            cd_address: start_sector,
            cd_length: sectors,
        }
    }

    pub fn write_lba(start_sector: u32, sectors: u16) -> CommandBlock {
        CommandBlock {
            tag: fastrand::u32(..),
            transfer_length: u32::from(sectors) * SECTOR_SIZE,
            flags: Direction::Out,
            lun: 0,
            cdb_length: 0xa,
            cd_code: CommandCode::WriteLBA,
            cd_address: start_sector,
            cd_length: sectors,
        }
    }

    pub fn tag(&self) -> u32 {
        self.tag
    }

    pub fn direction(&self) -> Direction {
        self.flags
    }

    pub fn transfer_length(&self) -> u32 {
        self.transfer_length
    }

    pub fn to_bytes(&self, mut bytes: &mut [u8]) -> usize {
        bytes.put_slice(b"USBC");
        bytes.put_u32(self.tag);
        bytes.put_u32(self.transfer_length);
        bytes.put_u8(self.flags.into());
        bytes.put_u8(self.lun);
        bytes.put_u8(self.cdb_length);
        bytes.put_u8(self.cd_code.into());
        bytes.put_u8(0);
        bytes.put_u32(self.cd_address);
        bytes.put_u8(0);
        bytes.put_u16(self.cd_length);
        COMMAND_BLOCK_BYTES
    }

    pub fn from_bytes(mut bytes: &[u8]) -> Result<CommandBlock, CommandBlockParseError> {
        if bytes.len() < COMMAND_BLOCK_BYTES {
            return Err(CommandBlockParseError::InvalidLength(bytes.len()));
        }
        let mut magic = [0u8; 4];
        bytes.copy_to_slice(&mut magic);
        if &magic != b"USBC" {
            return Err(CommandBlockParseError::InvalidSignature(magic));
        }
        let tag = bytes.get_u32();
        let transfer_length = bytes.get_u32();
        let flags = Direction::try_from(bytes.get_u8())
            .map_err(|e| CommandBlockParseError::UnknownFlags(e.number))?;
        let lun = bytes.get_u8();
        let cdb_length = bytes.get_u8();
        let cd_code = CommandCode::try_from(bytes.get_u8())
            .map_err(|e| CommandBlockParseError::UnknownCommandCode(e.number))?;
        bytes.advance(1);
        let cd_address = bytes.get_u32();
        bytes.advance(1);
        let cd_length = bytes.get_u16();
        Ok(CommandBlock {
            tag,
            transfer_length,
            flags,
            lun,
            cdb_length,
            cd_code,
            cd_address,
            cd_length,
        })
    }
}
#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn csw() {
        let c = CommandStatus {
            tag: 0x11223344,
            residue: 0x55667788,
            status: Status::SUCCESS,
        };
        let mut b = [0u8; 13];
        c.to_bytes(&mut b);
        let c2 = CommandStatus::from_bytes(&b).unwrap();
        assert_eq!(c, c2);
    }

    #[test]
    fn cbw() {
        let c = CommandBlock {
            tag: 0xdead,
            transfer_length: 0x11223344,
            flags: Direction::Out,
            lun: 0x66,
            cdb_length: 0x77,
            cd_code: CommandCode::EraseForce,
            cd_address: 0x11223344,
            cd_length: 0x5566,
        };
        let mut b = [0u8; 31];
        c.to_bytes(&mut b);
        let c2 = CommandBlock::from_bytes(&b).unwrap();
        assert_eq!(c, c2);
    }
}
