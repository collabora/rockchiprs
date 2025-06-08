use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow};
use clap::Parser;
use rockfile::boot::{
    RkBootEntry, RkBootEntryBytes, RkBootHeader, RkBootHeaderBytes, RkBootHeaderEntry,
};

fn parse_entry(header: RkBootHeaderEntry, name: &str, file: &mut File) -> Result<()> {
    for i in 0..header.count {
        let mut entry: RkBootEntryBytes = [0; 57];
        file.seek(SeekFrom::Start(
            header.offset as u64 + (header.size * i) as u64,
        ))?;
        file.read_exact(&mut entry)?;
        let entry = RkBootEntry::from_bytes(&entry);
        println!("== {} Entry  {} ==", name, i);
        println!("Name: {}", String::from_utf16(entry.name.as_slice())?);
        println!("Raw: {:?}", entry);

        let mut data = vec![0; entry.data_size as usize];
        file.seek(SeekFrom::Start(entry.data_offset as u64))?;
        file.read_exact(&mut data)?;

        let crc = crc::Crc::<u16>::new(&crc::CRC_16_IBM_3740);
        println!("Data CRC: {:x}", crc.checksum(&data));
    }

    Ok(())
}

fn parse_boot(path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header: RkBootHeaderBytes = [0; 102];
    file.read_exact(&mut header)?;
    let header =
        RkBootHeader::from_bytes(&header).ok_or_else(|| anyhow!("Failed to parse header"))?;

    println!("Raw Header: {:?}", header);
    println!(
        "chip: {:?} - {}",
        header.supported_chip,
        String::from_utf8_lossy(&header.supported_chip)
    );
    parse_entry(header.entry_471, "0x471", &mut file)?;
    parse_entry(header.entry_472, "0x472", &mut file)?;
    parse_entry(header.entry_loader, "loader", &mut file)?;
    Ok(())
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    BootFile { path: PathBuf },
}

#[derive(clap::Parser)]
struct Opts {
    #[command(subcommand)]
    command: Command,
}

fn main() -> Result<()> {
    let opt = Opts::parse();

    // Commands that don't talk a device
    match opt.command {
        Command::BootFile { path } => parse_boot(&path),
    }
}
