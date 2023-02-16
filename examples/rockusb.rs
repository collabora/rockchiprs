use std::{
    ffi::OsStr,
    fs::File,
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use bmap_parser::Bmap;
use clap::Parser;
use flate2::read::GzDecoder;
use rockusb::{
    bootfile::{RkBootEntry, RkBootEntryBytes, RkBootHeader, RkBootHeaderBytes, RkBootHeaderEntry},
    protocol::{ChipInfo, FlashInfo},
    FromOperation, UsbOperation,
};
use rusb::DeviceHandle;

fn find_device() -> Result<(DeviceHandle<rusb::GlobalContext>, u8, u8, u8)> {
    let devices = rusb::DeviceList::new()?;
    for d in devices.iter() {
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

                    match (input, output) {
                        (Some(input), Some(output)) => {
                            println!("Found {:?} interface: {}", d, i_desc.setting_number());
                            let handle = match d.open() {
                                Ok(h) => h,
                                _ => continue,
                            };

                            return Ok((
                                handle,
                                i_desc.setting_number(),
                                input.address(),
                                output.address(),
                            ));
                        }
                        (input, output) => {
                            println!(
                                "Device {:?} missing endpoints - {:?} {:?}",
                                d, input, output
                            );
                        }
                    }
                }
            }
        }
    }

    Err(anyhow!("No device found"))
}

struct LibUsbTransport {
    handle: DeviceHandle<rusb::GlobalContext>,
    interface: u8,
    ep_in: u8,
    ep_out: u8,
    offset: u32,
    written: usize,
    outstanding_write: usize,
    write_buffer: [u8; 512],
}

impl LibUsbTransport {
    fn new(
        handle: DeviceHandle<rusb::GlobalContext>,
        interface: u8,
        ep_in: u8,
        ep_out: u8,
    ) -> Self {
        Self {
            handle,
            interface,
            ep_in,
            ep_out,
            offset: 0,
            written: 0,
            outstanding_write: 0,
            write_buffer: [0u8; 512],
        }
    }

    fn handle_operation<T>(&mut self, mut operation: UsbOperation<T>) -> Result<T>
    where
        T: FromOperation,
        T: std::fmt::Debug,
    {
        loop {
            let step = operation.step();
            match step {
                rockusb::UsbStep::WriteBulk { data } => {
                    let _written = self
                        .handle
                        .write_bulk(self.ep_out, &data, Duration::from_secs(5))
                        .context("Failed to read")?;
                }
                rockusb::UsbStep::ReadBulk { mut data } => {
                    let _read = self
                        .handle
                        .read_bulk(self.ep_in, &mut data, Duration::from_secs(5))
                        .context("Failed to read")?;
                }
                rockusb::UsbStep::Finished(r) => break r.map_err(|e| e.into()),
            }
        }
    }

    fn flash_info(&mut self) -> Result<FlashInfo> {
        Ok(self.handle_operation(rockusb::flash_info())?)
    }

    fn chip_info(&mut self) -> Result<ChipInfo> {
        Ok(self.handle_operation(rockusb::chip_info())?)
    }

    pub fn read_lba(&mut self, start_sector: u32, read: &mut [u8]) -> Result<()> {
        self.handle_operation(rockusb::read_lba(start_sector, read))?;
        Ok(())
    }

    pub fn write_lba(&mut self, start_sector: u32, write: &[u8]) -> Result<()> {
        self.handle_operation(rockusb::write_lba(start_sector, write))?;
        Ok(())
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
            self.write_buffer[0..buf.len()].copy_from_slice(&buf);
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

fn read_flash_info() -> Result<()> {
    let (mut handle, interface, input, output) = find_device()?;
    handle.claim_interface(interface)?;
    let mut transport = LibUsbTransport::new(handle, interface, input, output);
    println!("Flash Info: {:0x?}", transport.flash_info()?);

    Ok(())
}

fn read_chip_info() -> Result<()> {
    let (mut handle, interface, input, output) = find_device()?;
    handle.claim_interface(interface)?;
    let mut transport = LibUsbTransport::new(handle, interface, input, output);
    println!("Chip Info: {:0x?}", transport.chip_info()?);

    Ok(())
}

fn read_lba(offset: u32, length: u16, path: &Path) -> Result<()> {
    let (mut handle, interface, input, output) = find_device()?;
    handle.claim_interface(interface)?;

    let mut data = Vec::new();
    data.resize(length as usize * 512, 0);

    let mut transport = LibUsbTransport::new(handle, interface, input, output);
    transport.read_lba(offset, &mut data)?;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(&data)?;
    Ok(())
}

fn write_lba(offset: u32, length: u16, path: &Path) -> Result<()> {
    let (mut handle, interface, input, output) = find_device()?;
    handle.claim_interface(interface)?;

    let mut data = Vec::new();
    data.resize(length as usize * 512, 0);

    let mut file = File::open(path)?;
    file.read_exact(&mut data)?;

    let mut transport = LibUsbTransport::new(handle, interface, input, output);
    transport.write_lba(offset, &mut data)?;

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

fn write_bmap(path: &Path) -> Result<()> {
    let (mut handle, interface, input, output) = find_device()?;
    handle.claim_interface(interface)?;

    let bmap_path = find_bmap(path).ok_or(anyhow!("Failed to find bmap"))?;
    println!("Using bmap file: {}", path.display());

    let mut bmap_file = File::open(bmap_path)?;
    let mut xml = String::new();
    bmap_file.read_to_string(&mut xml)?;
    let bmap = Bmap::from_xml(&xml)?;

    let transport = LibUsbTransport::new(handle, interface, input, output);
    // HACK to minimize small writes
    let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, transport);

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

        // Avoid splitting the 2 byte crc in two 4096 byte chunks
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

        // Send a single 0 byte to signal eof if every chunk was exactly 4096 bytes
        if data.len() % 4096 == 0 {
            handle.write_control(0x40, 0xc, 0, code, &[0u8], Duration::from_secs(5))?;
        }
        println!("Done!... waiting {}ms", entry.data_delay);
        if entry.data_delay > 0 {
            sleep(Duration::from_millis(entry.data_delay as u64));
        }
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
    FlashInfo,
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
        Command::Read {
            offset,
            length,
            path,
        } => read_lba(offset, length, &path),
        Command::Write {
            offset,
            length,
            path,
        } => write_lba(offset, length, &path),
        Command::WriteBmap { path } => write_bmap(&path),
        Command::ChipInfo => read_chip_info(),
        Command::FlashInfo => read_flash_info(),
    }
}
