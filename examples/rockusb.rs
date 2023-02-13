use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use rockusb::{
    CommandStatusBytes, RkBootEntry, RkBootEntryBytes, RkBootHeader, RkBootHeaderBytes,
    RkBootHeaderEntry,
};
use rusb::DeviceHandle;

fn find_device() -> Result<(DeviceHandle<rusb::GlobalContext>, u8, u8, u8)> {
    let devices = rusb::DeviceList::new()?;
    for d in devices.iter() {
        let handle = match d.open() {
            Ok(h) => h,
            _ => continue,
        };
        let desc = d.device_descriptor()?;
        if desc.vendor_id() != 0x2207 {
            continue;
        }

        for c in 0..desc.num_configurations() {
            let config = d.config_descriptor(c)?;
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
                    println!(" {:?} {:?}", input, output);

                    match (input, output) {
                        (Some(input), Some(output)) => {
                            println!("Found {:?} interface: {}", d, i_desc.setting_number());
                            return Ok((
                                handle,
                                i_desc.setting_number(),
                                input.address(),
                                output.address(),
                            ));
                        }
                        _ => (),
                    }
                }
            }
        }
    }

    Err(anyhow!("No device found"))
}

fn read_chip_info() -> Result<()> {
    let (mut handle, interface, input, output) = find_device()?;
    handle.claim_interface(interface)?;

    let cb = rockusb::CommandBlock {
        tag: 12,
        transfer_length: 16,
        flags: 0x80,    // Direction out
        cb_length: 0x6, // Eh?
        code: 0x1B,     // Read Chip Info
        ..Default::default()
    };
    let mut cb_bytes = Default::default();
    cb.to_bytes(&mut cb_bytes);

    println!("=> {:x?}", cb_bytes);
    handle
        .write_bulk(output, &cb_bytes, Duration::from_secs(5))
        .context("Failed to write")?;

    let mut info = [0u8; 16];
    let r = handle
        .read_bulk(input, &mut info, Duration::from_secs(5))
        .context("Failed to read info")?;
    println!("info => {:0x?}", info);

    let mut cs_bytes: CommandStatusBytes = Default::default();
    let r = handle
        .read_bulk(input, &mut cs_bytes, Duration::from_secs(5))
        .context("Failed to read csw")?;
    let cs = rockusb::CommandStatus::from_bytes(&cs_bytes);
    println!("=> {} - {:?} {:0x?}", r, cs, cs_bytes);
    Ok(())
}

fn parse_entry(header: RkBootHeaderEntry, name: &str, file: &mut File) -> Result<()> {
    for i in 0..header.count {
        let mut entry: RkBootEntryBytes = [0; 57];
        file.seek(SeekFrom::Start(
            header.offset as u64 + (header.size * i) as u64,
        ))?;
        file.read_exact(&mut entry)?;
        let entry = RkBootEntry::from_bytes(&mut entry);
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
    handle: &DeviceHandle<rusb::GlobalContext>,
) -> Result<()> {
    for i in 0..header.count {
        let mut entry: RkBootEntryBytes = [0; 57];
        file.seek(SeekFrom::Start(
            header.offset as u64 + (header.size * i) as u64,
        ))?;
        file.read_exact(&mut entry)?;
        let entry = RkBootEntry::from_bytes(&mut entry);
        println!("{} Name: {}", i, String::from_utf16(entry.name.as_slice())?);

        let mut data = Vec::new();
        data.resize(entry.data_size as usize, 0);
        file.seek(SeekFrom::Start(entry.data_offset as u64))?;
        file.read_exact(&mut data)?;

        if data.len() % 4096 == 4095 {
            data.push(0);
        }

        let crc = crc::Crc::<u16>::new(&crc::CRC_16_IBM_3740).checksum(&data);
        data.push((crc >> 8) as u8);
        data.push((crc & 0xff) as u8);
        println!("CRC: {:x}", crc);
        for chunk in data.chunks(4096) {
            handle.write_control(0x40, 0xc, 0, code, chunk, Duration::from_secs(5))?;
        }

        if data.len() % 4096 == 0 {
            handle.write_control(0x40, 0xc, 0, code, &[0u8], Duration::from_secs(5))?;
        }
        if entry.data_delay > 0 {
            sleep(Duration::from_millis(entry.data_delay as u64));
        }
        println!("Done!");
    }

    Ok(())
}

fn download_boot(path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header: RkBootHeaderBytes = [0; 102];
    file.read_exact(&mut header)?;

    let header =
        RkBootHeader::from_bytes(&header).ok_or_else(|| anyhow!("Failed to parse header"))?;

    let (mut handle, interface, input, output) = find_device()?;
    handle.claim_interface(interface)?;

    download_entry(header.entry_471, 0x471, &mut file, &mut handle)?;
    download_entry(header.entry_472, 0x472, &mut file, &mut handle)?;

    Ok(())
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    ParseBoot { path: PathBuf },
    DownloadBoot { path: PathBuf },
    ChipInfo,
}

#[derive(clap::Parser)]
struct Opts {
    #[command(subcommand)]
    command: Command,
}

fn main() -> Result<()> {
    let opt = Opts::parse();
    match opt.command {
        Command::ParseBoot { path } => parse_boot(&path),
        Command::DownloadBoot { path } => download_boot(&path),
        Command::ChipInfo => read_chip_info(),
    }
}
