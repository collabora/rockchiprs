use std::{
    ffi::OsStr,
    io::SeekFrom,
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

use anyhow::{Result, anyhow};
use async_compression::futures::bufread::GzipDecoder;
use bmap_parser::Bmap;
use clap::{Parser, ValueEnum};
use clap_num::maybe_hex;
use futures::io::{BufReader, BufWriter};
use rockfile::boot::{
    RkBootEntry, RkBootEntryBytes, RkBootHeader, RkBootHeaderBytes, RkBootHeaderEntry,
};
use rockusb::nusb::Device;
use rockusb::protocol::ResetOpcode;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};

async fn read_flash_info(mut device: Device) -> Result<()> {
    let info = device.flash_info().await?;
    println!("Raw Flash Info: {:0x?}", info);
    println!(
        "Flash size: {} MB ({} sectors)",
        info.sectors() / 2048,
        info.sectors()
    );

    Ok(())
}

async fn reset_device(mut device: Device, opcode: ResetOpcode) -> Result<()> {
    device.reset_device(opcode).await?;
    Ok(())
}

async fn read_chip_info(mut device: Device) -> Result<()> {
    println!("Chip Info: {:0x?}", device.chip_info().await?);
    Ok(())
}

async fn read_lba(mut device: Device, offset: u32, length: u16, path: &Path) -> Result<()> {
    let mut data = vec![0; length as usize * 512];
    device.read_lba(offset, &mut data).await?;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await?;
    file.write_all(&data).await?;
    Ok(())
}

async fn write_lba(mut device: Device, offset: u32, length: u16, path: &Path) -> Result<()> {
    let mut data = vec![0; length as usize * 512];

    let mut file = File::open(path).await?;
    file.read_exact(&mut data).await?;

    device.write_lba(offset, &data).await?;

    Ok(())
}

async fn read_file(device: Device, offset: u32, length: u16, path: &Path) -> Result<()> {
    let mut file = tokio::fs::File::create(path).await?;
    let mut io = device.into_io().await?.compat();

    io.seek(SeekFrom::Start(offset as u64 * 512)).await?;
    tokio::io::copy(&mut io.take(length as u64 * 512), &mut file).await?;
    Ok(())
}

async fn write_file(device: Device, offset: u32, path: &Path) -> Result<()> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut io = device.into_io().await?.compat();

    io.seek(SeekFrom::Start(offset as u64 * 512)).await?;
    tokio::io::copy(&mut file, &mut io).await?;
    Ok(())
}

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

async fn write_bmap(device: Device, path: &Path) -> Result<()> {
    let bmap_path = find_bmap(path).ok_or_else(|| anyhow!("Failed to find bmap"))?;
    println!("Using bmap file: {}", path.display());

    let mut bmap_file = File::open(bmap_path).await?;
    let mut xml = String::new();
    bmap_file.read_to_string(&mut xml).await?;
    let bmap = Bmap::from_xml(&xml)?;

    // HACK to minimize small writes
    let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, device.into_io().await?);

    let file = File::open(path).await?;
    let mut file = BufReader::with_capacity(16 * 1024 * 1024, file.compat());
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

async fn download_entry(
    header: RkBootHeaderEntry,
    code: u16,
    file: &mut File,
    device: &mut Device,
) -> Result<()> {
    for i in 0..header.count {
        let mut entry: RkBootEntryBytes = [0; 57];

        file.seek(SeekFrom::Start(
            header.offset as u64 + (header.size * i) as u64,
        ))
        .await?;
        file.read_exact(&mut entry).await?;

        let entry = RkBootEntry::from_bytes(&entry);
        println!("{} Name: {}", i, String::from_utf16(entry.name.as_slice())?);

        let mut data = vec![0; entry.data_size as usize];

        file.seek(SeekFrom::Start(entry.data_offset as u64)).await?;
        file.read_exact(&mut data).await?;

        device.write_maskrom_area(code, &data).await?;

        println!("Done!... waiting {}ms", entry.data_delay);
        if entry.data_delay > 0 {
            sleep(Duration::from_millis(entry.data_delay as u64));
        }
    }

    Ok(())
}

async fn download_boot(mut device: Device, path: &Path) -> Result<()> {
    let mut file = File::open(path).await?;
    let mut header: RkBootHeaderBytes = [0; 102];
    file.read_exact(&mut header).await?;

    let header =
        RkBootHeader::from_bytes(&header).ok_or_else(|| anyhow!("Failed to parse header"))?;

    download_entry(header.entry_471, 0x471, &mut file, &mut device).await?;
    download_entry(header.entry_472, 0x472, &mut file, &mut device).await?;

    Ok(())
}

#[derive(Debug, clap::Parser)]
enum Command {
    List,
    DownloadBoot {
        path: PathBuf,
    },
    Read {
        #[clap(value_parser=maybe_hex::<u32>)]
        offset: u32,
        #[clap(value_parser=maybe_hex::<u16>)]
        length: u16,
        path: PathBuf,
    },
    ReadFile {
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
    ResetDevice {
        #[clap(value_enum, default_value_t=ArgResetOpcode::Reset)]
        opcode: ArgResetOpcode,
    },
}

#[derive(ValueEnum, Clone, Debug)]
pub enum ArgResetOpcode {
    /// Reset
    Reset,
    /// Reset to USB mass-storage device class
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
struct DeviceArg {
    bus_number: u8,
    address: u8,
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
struct Opts {
    #[arg(short, long, value_parser = parse_device)]
    /// Device type specified as <bus>:<address>
    device: Option<DeviceArg>,
    #[command(subcommand)]
    command: Command,
}

fn list_available_devices() -> Result<()> {
    let devices = rockusb::nusb::devices()?;
    println!("Available rockchip devices:");
    for d in devices {
        println!(
            "* Bus {} Device {} ID {}:{}",
            d.bus_number(),
            d.device_address(),
            d.vendor_id(),
            d.product_id()
        );
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opts::parse();

    // Commands that don't talk a device
    if matches!(opt.command, Command::List) {
        return list_available_devices();
    }

    let mut devices = rockusb::nusb::devices()?;
    let info = if let Some(dev) = opt.device {
        devices
            .find(|d| d.bus_number() == dev.bus_number && d.device_address() == dev.address)
            .ok_or_else(|| anyhow!("Specified device not found"))?
    } else {
        let mut devices: Vec<_> = devices.collect();
        match devices.len() {
            0 => Err(anyhow!("No devices found")),
            1 => Ok(devices.pop().unwrap()),
            _ => {
                drop(devices);
                let _ = list_available_devices();
                println!();
                Err(anyhow!(
                    "Please select a specific device using the -d option"
                ))
            }
        }?
    };

    let mut device = Device::from_usb_device_info(info)?;

    match opt.command {
        Command::List => unreachable!(),
        Command::DownloadBoot { path } => download_boot(device, &path).await,
        Command::Read {
            offset,
            length,
            path,
        } => read_lba(device, offset, length, &path).await,
        Command::ReadFile {
            offset,
            length,
            path,
        } => read_file(device, offset, length, &path).await,
        Command::Write {
            offset,
            length,
            path,
        } => write_lba(device, offset, length, &path).await,
        Command::WriteFile { offset, path } => write_file(device, offset, &path).await,
        Command::WriteBmap { path } => write_bmap(device, &path).await,
        Command::ChipInfo => read_chip_info(device).await,
        Command::FlashId => {
            let id = device.flash_id().await?;
            println!("Flash id: {}", id.to_str());
            println!("raw: {:?}", id);
            Ok(())
        }
        Command::FlashInfo => read_flash_info(device).await,
        Command::ResetDevice { opcode } => reset_device(device, opcode.into()).await,
    }
}
