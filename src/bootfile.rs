use bytes::Buf;

pub type RkTimeBytes = [u8; 7];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RkTime {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

impl RkTime {
    pub fn from_bytes(bytes: &RkTimeBytes) -> RkTime {
        let mut bytes = &bytes[..];
        let year = bytes.get_u16_le();
        let month = bytes.get_u8();
        let day = bytes.get_u8();
        let hour = bytes.get_u8();
        let minute = bytes.get_u8();
        let second = bytes.get_u8();
        RkTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }
}

pub type RkBootHeaderEntryBytes = [u8; 6];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RkBootHeaderEntry {
    pub count: u8,
    pub offset: u32,
    pub size: u8,
}
impl RkBootHeaderEntry {
    pub fn from_bytes(bytes: &RkBootHeaderEntryBytes) -> RkBootHeaderEntry {
        let mut bytes = &bytes[..];

        let count = bytes.get_u8();
        let offset = bytes.get_u32_le();
        let size = bytes.get_u8();

        RkBootHeaderEntry {
            count,
            offset,
            size,
        }
    }
}

pub type RkBootEntryBytes = [u8; 57];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RkBootEntry {
    pub size: u8,
    pub type_: u32,
    pub name: [u16; 20],
    pub data_offset: u32,
    pub data_size: u32,
    pub data_delay: u32,
}

impl RkBootEntry {
    pub fn from_bytes(bytes: &RkBootEntryBytes) -> RkBootEntry {
        let mut bytes = &bytes[..];

        let size = bytes.get_u8();
        let type_ = bytes.get_u32_le();
        let mut name = [0u16; 20];
        for n in &mut name {
            *n = bytes.get_u16_le()
        }
        let data_offset = bytes.get_u32_le();
        let data_size = bytes.get_u32_le();
        let data_delay = bytes.get_u32_le();

        RkBootEntry {
            size,
            type_,
            name,
            data_offset,
            data_size,
            data_delay,
        }
    }
}

pub type RkBootHeaderBytes = [u8; 102];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RkBootHeader {
    pub tag: [u8; 4],
    pub size: u16,
    pub version: u32,
    pub merge_version: u32,
    pub release: RkTime,
    pub supported_chip: u32,
    pub entry_471: RkBootHeaderEntry,
    pub entry_472: RkBootHeaderEntry,
    pub entry_loader: RkBootHeaderEntry,
    pub sign_flag: u8,
    pub rc4_flag: u8,
}

impl RkBootHeader {
    pub fn from_bytes(bytes: &RkBootHeaderBytes) -> Option<RkBootHeader> {
        let mut bytes = &bytes[..];
        let mut tag = [0u8; 4];
        bytes.copy_to_slice(&mut tag);

        if &tag != b"BOOT" && &tag != b"LDR " {
            return None;
        }
        let size = bytes.get_u16_le();
        let version = bytes.get_u32_le();
        let merge_version = bytes.get_u32_le();

        let release = RkTime::from_bytes(bytes[0..7].try_into().unwrap());
        bytes.advance(7);

        let supported_chip = bytes.get_u32_le();

        let entry_471 = RkBootHeaderEntry::from_bytes(bytes[0..6].try_into().unwrap());
        bytes.advance(6);
        let entry_472 = RkBootHeaderEntry::from_bytes(bytes[0..6].try_into().unwrap());
        bytes.advance(6);
        let entry_loader = RkBootHeaderEntry::from_bytes(bytes[0..6].try_into().unwrap());
        bytes.advance(6);

        let sign_flag = bytes.get_u8();
        let rc4_flag = bytes.get_u8();

        Some(RkBootHeader {
            tag,
            size,
            version,
            merge_version,
            release,
            supported_chip,
            entry_471,
            entry_472,
            entry_loader,
            sign_flag,
            rc4_flag,
        })
    }
}
