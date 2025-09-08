use std::{
    ffi::OsStr,
    fs::File,
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

use anyhow::{Result, anyhow, ensure};
use bmap_parser::Bmap;
use clap::ValueEnum;
use clap_num::maybe_hex;
use flate2::read::GzDecoder;
use rockfile::boot::{
    RkBootEntry, RkBootEntryBytes, RkBootHeader, RkBootHeaderBytes, RkBootHeaderEntry,
};
use rockusb::{
    device::{Device, Transport},
    protocol::ResetOpcode,
};

#[cfg(feature = "async")]
use async_compression::futures::bufread::GzipDecoder;
#[cfg(feature = "async")]
use rockusb::device::{DeviceAsync, TransportAsync};
#[cfg(feature = "async")]
use tokio::io::{AsyncReadExt, AsyncSeekExt};
#[cfg(feature = "async")]
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};

fn find_bmap(img: &Path) -> Option<PathBuf> {
    fn append(path: PathBuf) -> PathBuf {
        let mut p = path.into_os_string();
        p.push(".bmap");
        p.into()
    }

    let mut bmap = img.to_path_buf();
    loop {
        bmap = append(bmap);
        if bmap.exists() {
            return Some(bmap);
        }
        // Drop .bmap
        bmap.set_extension("");
        bmap.extension()?;
        // Drop existing orignal extension part
        bmap.set_extension("");
    }
}

#[maybe_async_cfg::maybe(
    sync(keep_self),
    async(feature = "async", idents(Device(async = "DeviceAsync")))
)]
#[allow(dead_code)]
pub struct ExampleDevice<T> {
    device: Device<T>,
}

#[maybe_async_cfg::maybe(
    sync(keep_self),
    async(
        feature = "async",
        idents(Device(async = "DeviceAsync"), Transport(async = "TransportAsync"))
    )
)]
#[allow(dead_code)]
impl<T> ExampleDevice<T>
where
    T: Transport + Send + Unpin + 'static,
{
    pub fn new(device: Device<T>) -> Self {
        Self { device }
    }

    pub async fn read_flash_info(&mut self) -> Result<()> {
        let info = self.device.flash_info().await?;
        println!("Raw Flash Info: {:0x?}", info);
        println!(
            "Flash size: {} MB ({} sectors)",
            info.sectors() / 2048,
            info.sectors()
        );

        Ok(())
    }

    pub async fn read_flash_id(&mut self) -> Result<()> {
        let id = self.device.flash_id().await?;
        println!("Flash id: {}", id.to_str());
        println!("raw: {:?}", id);
        Ok(())
    }

    pub async fn read_capability(&mut self) -> Result<()> {
        let capability = self.device.capability().await?;
        println!("Raw Capability: {:0x?}", capability);
        println!("Capability:");
        if capability.direct_lba() {
            println!(" - Direct LBA");
        }

        if capability.vendor_storage() {
            println!(" - Vendor storage");
        }

        if capability.first_4m_access() {
            println!(" - First 4M Access");
        }

        if capability.read_lba() {
            println!(" - Read LBA");
        }

        if capability.read_com_log() {
            println!(" - Read COM log");
        }

        if capability.read_idb_config() {
            println!(" - Read IDB config");
        }

        if capability.read_secure_mode() {
            println!(" - Read secure mode");
        }

        if capability.new_idb() {
            println!(" - New IDB");
        }

        Ok(())
    }

    pub async fn erase_flash(&mut self) -> Result<()> {
        static MAX_DIRECT_ERASE: u32 = 1024;
        static MAX_LBA_ERASE: u32 = 32 * 1024;

        // Get flash info
        let flash_info = self.device.flash_info().await?;

        ensure!(flash_info.sectors() > 0, "Invalid flash chip");

        // Get flash id
        let flash_id = self.device.flash_id().await?;
        let is_emmc = flash_id.to_str() == "EMMC ";

        // Get flash capability
        let capability = self.device.capability().await?;

        let is_lba = capability.direct_lba();

        let mut blocks_left = flash_info.sectors();
        let mut first = 0;

        /*
         * Different types of memory need more or less time to erase blocks.
         * Limit the number of blocks to avoid hitting the USB command
         * timeout.
         *
         * rkdeveloptool uses those values too, as they are also useful to show erase
         * progress.
         */
        let max_blocks = if is_emmc || is_lba {
            MAX_LBA_ERASE
        } else {
            MAX_DIRECT_ERASE
        };

        while blocks_left > 0 {
            let count = blocks_left.min(max_blocks);

            if is_emmc || is_lba {
                self.device.erase_lba(first, count as u16).await?;
            } else {
                self.device.erase_force(first, count as u16).await?;
            }

            blocks_left -= count;
            first += count;
        }

        Ok(())
    }

    pub async fn read_storage(&mut self) -> Result<()> {
        let storage = self.device.storage().await?;
        println!("Raw Storage: {:0x?}", storage);
        Ok(())
    }

    pub async fn change_storage(&mut self, target: u8) -> Result<()> {
        self.device.change_storage(target).await?;
        Ok(())
    }

    pub async fn reset_device(&mut self, opcode: ResetOpcode) -> Result<()> {
        self.device.reset_device(opcode).await?;
        Ok(())
    }

    pub async fn read_chip_info(&mut self) -> Result<()> {
        println!("Chip Info: {:0x?}", self.device.chip_info().await?);
        Ok(())
    }

    pub async fn read_lba(&mut self, offset: u32, length: u16, path: &Path) -> Result<()> {
        let mut data = vec![0; length as usize * 512];
        self.device.read_lba(offset, &mut data).await?;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        file.write_all(&data)?;
        Ok(())
    }

    pub async fn write_lba(&mut self, offset: u32, length: u16, path: &Path) -> Result<()> {
        let mut data = vec![0; length as usize * 512];

        let mut file = File::open(path)?;
        file.read_exact(&mut data)?;

        self.device.write_lba(offset, &data).await?;

        Ok(())
    }

    #[maybe_async_cfg::only_if(sync)]
    pub fn write_file(self, offset: u32, path: &Path) -> Result<()> {
        let mut file = File::open(path)?;
        let mut io = self.device.into_io().await?;

        io.seek(SeekFrom::Start(offset as u64 * 512))?;
        std::io::copy(&mut file, &mut io)?;
        Ok(())
    }

    #[maybe_async_cfg::only_if(async)]
    pub async fn write_file(self, offset: u32, path: &Path) -> Result<()> {
        let mut file = tokio::fs::File::open(path).await?;
        let mut io = self.device.into_io().await?.compat();

        io.seek(SeekFrom::Start(offset as u64 * 512)).await?;
        tokio::io::copy(&mut file, &mut io).await?;
        Ok(())
    }

    #[maybe_async_cfg::only_if(sync)]
    pub fn write_bmap(self, path: &Path) -> Result<()> {
        let bmap_path = find_bmap(path).ok_or_else(|| anyhow!("Failed to find bmap"))?;
        println!("Using bmap file: {}", bmap_path.display());

        let mut bmap_file = File::open(bmap_path)?;
        let mut xml = String::new();
        bmap_file.read_to_string(&mut xml)?;
        let bmap = Bmap::from_xml(&xml)?;

        // HACK to minimize small writes
        let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, self.device.into_io()?);

        let mut file = File::open(path)?;
        match path.extension().and_then(OsStr::to_str) {
            Some("gz") => {
                let gz = GzDecoder::new(file);
                let mut gz = bmap_parser::Discarder::new(gz);
                bmap_parser::copy(&mut gz, &mut writer, &bmap)?;
            }
            _ => {
                bmap_parser::copy(&mut file, &mut writer, &bmap)?;
            }
        }

        Ok(())
    }

    #[maybe_async_cfg::only_if(async)]
    pub async fn write_bmap(self, path: &Path) -> Result<()> {
        let bmap_path = find_bmap(path).ok_or_else(|| anyhow!("Failed to find bmap"))?;
        println!("Using bmap file: {}", bmap_path.display());

        let mut bmap_file = tokio::fs::File::open(bmap_path).await?;
        let mut xml = String::new();
        bmap_file.read_to_string(&mut xml).await?;
        let bmap = Bmap::from_xml(&xml)?;

        // HACK to minimize small writes
        let mut writer =
            futures::io::BufWriter::with_capacity(16 * 1024 * 1024, self.device.into_io().await?);

        let file = tokio::fs::File::open(path).await?;
        let mut file = futures::io::BufReader::with_capacity(16 * 1024 * 1024, file.compat());
        match path.extension().and_then(OsStr::to_str) {
            Some("gz") => {
                let gz = GzipDecoder::new(file);
                let mut gz = bmap_parser::AsyncDiscarder::new(gz);
                bmap_parser::copy_async(&mut gz, &mut writer, &bmap).await?;
            }
            _ => {
                bmap_parser::copy_async(&mut file, &mut writer, &bmap).await?;
            }
        }
        Ok(())
    }

    pub async fn download_entry(
        &mut self,
        header: RkBootHeaderEntry,
        code: u16,
        file: &mut File,
    ) -> Result<()> {
        for i in 0..header.count {
            let mut entry: RkBootEntryBytes = [0; 57];

            file.seek(SeekFrom::Start(
                header.offset as u64 + (header.size * i) as u64,
            ))?;
            file.read_exact(&mut entry)?;

            let entry = RkBootEntry::from_bytes(&entry);
            println!("{} Name: {}", i, String::from_utf16(entry.name.as_slice())?);

            let mut data = vec![0; entry.data_size as usize];

            file.seek(SeekFrom::Start(entry.data_offset as u64))?;
            file.read_exact(&mut data)?;

            self.device.write_maskrom_area(code, &data).await?;

            println!("Done!... waiting {}ms", entry.data_delay);
            if entry.data_delay > 0 {
                sleep(Duration::from_millis(entry.data_delay as u64));
            }
        }

        Ok(())
    }

    pub async fn download_boot(&mut self, path: &Path) -> Result<()> {
        let mut file = File::open(path)?;
        let mut header: RkBootHeaderBytes = [0; 102];
        file.read_exact(&mut header)?;

        let header =
            RkBootHeader::from_bytes(&header).ok_or_else(|| anyhow!("Failed to parse header"))?;

        self.download_entry(header.entry_471, 0x471, &mut file)
            .await?;
        self.download_entry(header.entry_472, 0x472, &mut file)
            .await?;

        Ok(())
    }

    pub async fn download_maskrom_area(&mut self, area: u16, path: &Path) -> Result<()> {
        let data = std::fs::read(path)?;
        self.device.write_maskrom_area(area, &data).await?;
        Ok(())
    }
}

#[derive(Debug, clap::Parser)]
pub enum Command {
    /// List rockchip devices in rockusb mode
    List,
    /// Download boot code from a rockfile (maskrom mode)
    DownloadBoot {
        path: PathBuf,
    },
    /// Download code to sram area (maskrom mode)
    DownloadSram {
        path: PathBuf,
    },
    /// Download code to DDR area (maskrom mode)
    DownloadDDR {
        path: PathBuf,
    },
    Read {
        #[clap(value_parser=maybe_hex::<u32>)]
        offset: u32,
        #[clap(value_parser=maybe_hex::<u16>)]
        length: u16,
        path: PathBuf,
    },
    Write {
        #[clap(value_parser=maybe_hex::<u32>)]
        offset: u32,
        #[clap(value_parser=maybe_hex::<u16>)]
        length: u16,
        path: PathBuf,
    },
    WriteFile {
        #[clap(value_parser=maybe_hex::<u32>)]
        offset: u32,
        path: PathBuf,
    },
    WriteBmap {
        path: PathBuf,
    },
    ChipInfo,
    FlashId,
    FlashInfo,
    Capability,
    EraseFlash,
    Storage,
    ChangeStorage {
        target: u8,
    },
    ResetDevice {
        #[clap(value_enum, default_value_t=ArgResetOpcode::Reset)]
        opcode: ArgResetOpcode,
    },
}

impl Command {
    #[maybe_async_cfg::maybe(
        sync(keep_self),
        async(
            feature = "async",
            idents(
                ExampleDevice(async = "ExampleDeviceAsync"),
                Transport(async = "TransportAsync")
            )
        )
    )]
    #[allow(dead_code)]
    pub async fn run<T>(self, mut device: ExampleDevice<T>) -> Result<()>
    where
        T: Transport + Send + Unpin + 'static,
    {
        match self {
            Command::List => unreachable!(),
            Command::DownloadSram { path } => device.download_maskrom_area(0x471, &path).await,
            Command::DownloadDDR { path } => device.download_maskrom_area(0x472, &path).await,
            Command::DownloadBoot { path } => device.download_boot(&path).await,
            Command::Read {
                offset,
                length,
                path,
            } => device.read_lba(offset, length, &path).await,
            Command::Write {
                offset,
                length,
                path,
            } => device.write_lba(offset, length, &path).await,
            Command::WriteFile { offset, path } => device.write_file(offset, &path).await,
            Command::WriteBmap { path } => device.write_bmap(&path).await,
            Command::ChipInfo => device.read_chip_info().await,
            Command::FlashId => device.read_flash_id().await,
            Command::FlashInfo => device.read_flash_info().await,
            Command::EraseFlash => device.erase_flash().await,
            Command::Capability => device.read_capability().await,
            Command::Storage => device.read_storage().await,
            Command::ChangeStorage { target } => device.change_storage(target).await,
            Command::ResetDevice { opcode } => device.reset_device(opcode.into()).await,
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum ArgResetOpcode {
    /// Reset
    Reset,
    /// Reset to USB mass-storage device class
    #[allow(clippy::upper_case_acronyms)]
    MSC,
    /// Powers the SOC off
    PowerOff,
    /// Reset to maskrom mode
    Maskrom,
    /// Disconnect from USB
    Disconnect,
}

impl From<ArgResetOpcode> for ResetOpcode {
    fn from(arg: ArgResetOpcode) -> ResetOpcode {
        match arg {
            ArgResetOpcode::Reset => ResetOpcode::Reset,
            ArgResetOpcode::MSC => ResetOpcode::MSC,
            ArgResetOpcode::PowerOff => ResetOpcode::PowerOff,
            ArgResetOpcode::Maskrom => ResetOpcode::Maskrom,
            ArgResetOpcode::Disconnect => ResetOpcode::Disconnect,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceArg {
    pub bus_number: u8,
    pub address: u8,
}

fn parse_device(device: &str) -> Result<DeviceArg> {
    let mut parts = device.split(':');
    let bus_number = parts
        .next()
        .ok_or_else(|| anyhow!("No bus number: use <bus>:<address>"))?
        .parse()
        .map_err(|_| anyhow!("Bus should be a number"))?;
    let address = parts
        .next()
        .ok_or_else(|| anyhow!("No address: use <bus>:<address>"))?
        .parse()
        .map_err(|_| anyhow!("Address should be a numbrer"))?;
    if parts.next().is_some() {
        return Err(anyhow!("Too many parts"));
    }
    Ok(DeviceArg {
        bus_number,
        address,
    })
}

#[derive(clap::Parser)]
pub struct Opts {
    #[arg(short, long, value_parser = parse_device)]
    /// Device type specified as <bus>:<address>
    pub device: Option<DeviceArg>,
    #[command(subcommand)]
    pub command: Command,
}
