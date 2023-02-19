use std::{
    ffi::OsStr,
    fs::File,
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

use anyhow::{anyhow, Result};
use bmap_parser::Bmap;
use clap::Parser;
use flate2::read::GzDecoder;
use rockfile::boot::{
    RkBootEntry, RkBootEntryBytes, RkBootHeader, RkBootHeaderBytes, RkBootHeaderEntry,
};
use rockusb::libusb::{DeviceUnavalable, LibUsbTransport};

fn read_flash_info(mut transport: LibUsbTransport) -> Result<()> {
    let info = transport.flash_info()?;
    println!("Raw Flash Info: {:0x?}", info);
    println!(
        "Flash size: {} MB ({} sectors)",
        info.sectors() / 2048,
        info.sectors()
    );

    Ok(())
}

fn read_chip_info(mut transport: LibUsbTransport) -> Result<()> {
    println!("Chip Info: {:0x?}", transport.chip_info()?);
    Ok(())
}

fn read_lba(mut transport: LibUsbTransport, offset: u32, length: u16, path: &Path) -> Result<()> {
    let mut data = Vec::new();
    data.resize(length as usize * 512, 0);
    transport.read_lba(offset, &mut data)?;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(&data)?;
    Ok(())
}

fn write_lba(mut transport: LibUsbTransport, offset: u32, length: u16, path: &Path) -> Result<()> {
    let mut data = Vec::new();
    data.resize(length as usize * 512, 0);

    let mut file = File::open(path)?;
    file.read_exact(&mut data)?;

    transport.write_lba(offset, &data)?;

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

fn write_bmap(transport: LibUsbTransport, path: &Path) -> Result<()> {
    let bmap_path = find_bmap(path).ok_or_else(|| anyhow!("Failed to find bmap"))?;
    println!("Using bmap file: {}", path.display());

    let mut bmap_file = File::open(bmap_path)?;
    let mut xml = String::new();
    bmap_file.read_to_string(&mut xml)?;
    let bmap = Bmap::from_xml(&xml)?;

    // HACK to minimize small writes
    let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, transport.into_io()?);

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

fn parse_entry(header: RkBootHeaderEntry, name: &str, file: &mut File) -> Result<()> {
    for i in 0..header.count {
        let mut entry: RkBootEntryBytes = [0; 57];
        file.seek(SeekFrom::Start(
            header.offset as u64 + (header.size * i) as u64,
        ))?;
        file.read_exact(&mut entry)?;
        let entry = RkBootEntry::from_bytes(&entry);
        println!("== {} Entry  {} ==", name, i);
        println!("{:?}", entry);
        println!("Name: {}", String::from_utf16(entry.name.as_slice())?);

        let mut data = Vec::new();
        data.resize(entry.data_size as usize, 0);
        file.seek(SeekFrom::Start(entry.data_offset as u64))?;
        file.read_exact(&mut data)?;

        let crc = crc::Crc::<u16>::new(&crc::CRC_16_IBM_3740);
        println!("CRC: {:x}", crc.checksum(&data));
    }

    Ok(())
}

fn parse_boot(path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header: RkBootHeaderBytes = [0; 102];
    file.read_exact(&mut header)?;
    let header =
        RkBootHeader::from_bytes(&header).ok_or_else(|| anyhow!("Failed to parse header"))?;

    println!("Header: {:?}", header);
    println!(
        "chip: {:x} - {}",
        header.supported_chip,
        String::from_utf8_lossy(&header.supported_chip.to_le_bytes())
    );
    parse_entry(header.entry_471, "0x471", &mut file)?;
    parse_entry(header.entry_472, "0x472", &mut file)?;
    parse_entry(header.entry_loader, "loader", &mut file)?;
    Ok(())
}

fn download_entry(
    header: RkBootHeaderEntry,
    code: u16,
    file: &mut File,
    transport: &mut LibUsbTransport,
) -> Result<()> {
    for i in 0..header.count {
        let mut entry: RkBootEntryBytes = [0; 57];

        file.seek(SeekFrom::Start(
            header.offset as u64 + (header.size * i) as u64,
        ))?;
        file.read_exact(&mut entry)?;

        let entry = RkBootEntry::from_bytes(&entry);
        println!("{} Name: {}", i, String::from_utf16(entry.name.as_slice())?);

        let mut data = Vec::new();
        data.resize(entry.data_size as usize, 0);

        file.seek(SeekFrom::Start(entry.data_offset as u64))?;
        file.read_exact(&mut data)?;

        transport.write_maskrom_area(code, &data)?;

        println!("Done!... waiting {}ms", entry.data_delay);
        if entry.data_delay > 0 {
            sleep(Duration::from_millis(entry.data_delay as u64));
        }
    }

    Ok(())
}

fn download_boot(mut transport: LibUsbTransport, path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header: RkBootHeaderBytes = [0; 102];
    file.read_exact(&mut header)?;

    let header =
        RkBootHeader::from_bytes(&header).ok_or_else(|| anyhow!("Failed to parse header"))?;

    download_entry(header.entry_471, 0x471, &mut file, &mut transport)?;
    download_entry(header.entry_472, 0x472, &mut file, &mut transport)?;

    Ok(())
}

fn run_nbd(transport: LibUsbTransport) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:10809").unwrap();

    println!(
        "Listening for nbd connection on: {:?}",
        listener.local_addr()?
    );

    let mut stream = listener
        .incoming()
        .next()
        .transpose()?
        .ok_or_else(|| anyhow!("Connection failure"))?;
    // Stop listening for new connections
    drop(listener);

    let io = transport.into_io()?;

    let export = nbd::Export {
        size: io.size(),
        readonly: false,
        ..Default::default()
    };

    println!("Connection!");

    nbd::server::handshake(&mut stream, &export)?;
    println!("Shook hands!");
    nbd::server::transmission(&mut stream, io)?;
    println!("nbd client disconnected");
    Ok(())
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    List,
    ParseBoot {
        path: PathBuf,
    },
    DownloadBoot {
        path: PathBuf,
    },
    Read {
        offset: u32,
        length: u16,
        path: PathBuf,
    },
    Write {
        offset: u32,
        length: u16,
        path: PathBuf,
    },
    WriteBmap {
        path: PathBuf,
    },
    ChipInfo,
    FlashId,
    FlashInfo,
    // Run/expose device as a network block device
    Nbd,
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
    let devices = rockusb::libusb::Devices::new()?;
    println!("Available rockchip devices");
    for d in devices.iter() {
        match d {
            Ok(mut d) => println!("* {:?}", d.handle().device()),
            Err(DeviceUnavalable { device, error }) => {
                println!("* {:?} - Unavailable: {}", device, error)
            }
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let opt = Opts::parse();

    // Commands that don't talk a device
    match opt.command {
        Command::ParseBoot { path } => return parse_boot(&path),
        Command::List => return list_available_devices(),
        _ => (),
    }

    let devices = rockusb::libusb::Devices::new()?;
    let mut transport = if let Some(dev) = opt.device {
        devices
            .iter()
            .find(|d| match d {
                Ok(device) => {
                    device.bus_number() == dev.bus_number && device.address() == dev.address
                }
                Err(DeviceUnavalable { device, .. }) => {
                    device.bus_number() == dev.bus_number && device.address() == dev.address
                }
            })
            .ok_or_else(|| anyhow!("Specified device not found"))?
    } else {
        let mut devices: Vec<_> = devices.iter().collect();
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
    }?;

    match opt.command {
        Command::ParseBoot { .. } | Command::List => unreachable!(),
        Command::DownloadBoot { path } => download_boot(transport, &path),
        Command::Read {
            offset,
            length,
            path,
        } => read_lba(transport, offset, length, &path),
        Command::Write {
            offset,
            length,
            path,
        } => write_lba(transport, offset, length, &path),
        Command::WriteBmap { path } => write_bmap(transport, &path),
        Command::ChipInfo => read_chip_info(transport),
        Command::FlashId => {
            let id = transport.flash_id()?;
            println!("Flash id: {}", id.to_str());
            println!("raw: {:?}", id);
            Ok(())
        }
        Command::FlashInfo => read_flash_info(transport),
        Command::Nbd => run_nbd(transport),
    }
}
