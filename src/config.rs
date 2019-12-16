/* SPDX-License-Identifier: MIT */

use crate::generator::run_generator;
use crate::ResultExt;
use failure::Error;
use ini::Ini;
use std::borrow::Cow;
use std::env;
use std::fs;
use std::io::{prelude::*, BufReader};
use std::iter::FromIterator;
use std::path::{self, Path, PathBuf};


pub struct Device {
    pub name: String,
    pub memory_limit_mb: u64,
    pub zram_fraction: f64,
    pub disksize: u64,
}

impl Device {
    fn new(name: String) -> Device {
        Device {
            name,
            memory_limit_mb: 2 * 1024,
            zram_fraction: 0.25,
            disksize: 0,
        }
    }
}


pub struct Config {
    pub root: Cow<'static, str>,
    pub devices: Vec<Device>,
    pub module: ModuleConfig,
}

pub enum ModuleConfig {
    Generator { output_directory: PathBuf },
    DeviceSetup { name: String },
}


impl Config {
    pub fn parse() -> Result<Config, Error> {
        let root: Cow<'static, str> =
            env::var("ZRAM_GENERATOR_ROOT").map(|mut root| {
                if !root.ends_with(path::is_separator) {
                    root.push('/');
                }
                println!("Using {:?} as root directory", root);
                root.into()
            }).unwrap_or("/".into());

        let mut args = env::args().skip(1);
        let module = match args.next() {
            Some(outdir) => {
                match &outdir[..] {
                    "--setup-device" =>
                        ModuleConfig::DeviceSetup {
                            name: args.next()
                                      .filter(|dev| &dev[0..4] == "zram")
                                      .ok_or_else(|| failure::err_msg("--setup-device requires device argument"))?
                        },
                    _ =>
                        match (args.next(), args.next(), args.next()) {
                            (Some(_), Some(_), None) |
                            (None, None, None) =>
                                ModuleConfig::Generator { output_directory: PathBuf::from(outdir) },
                            _ =>
                                return Err(failure::err_msg("This program requires 1 or 3 arguments")),
                        }
                }
            }
            None => return Err(failure::err_msg("This program requires 1 or 3 arguments")),
        };

        let devices = Config::read_devices(&root)?;
        Ok(Config { root, devices, module })
    }

    fn read_devices(root: &str) -> Result<Vec<Device>, Error> {
        let path = Path::new(root).join("etc/systemd/zram-generator.conf");
        if !path.exists() {
            println!("No configuration file found.");
            return Ok(vec![]);
        }

        let memtotal_mb = get_total_memory_kb(&root)? as f64 / 1024.;

        Result::from_iter(Ini::load_from_file(&path).with_path(&path)?.into_iter().map(|(section_name, section)| {
            let section_name = section_name.map(Cow::Owned).unwrap_or(Cow::Borrowed("(no title)"));

            if !section_name.starts_with("zram") {
                println!("Ignoring section \"{}\"", section_name);
                return Ok(None);
            }

            let mut dev = Device::new(section_name.into_owned());

            if let Some(val) = section.get("memory-limit") {
                if val == "none" {
                    dev.memory_limit_mb = u64::max_value();
                } else {
                    dev.memory_limit_mb = val.parse()
                        .map_err(|e| format_err!("Failed to parse memory-limit \"{}\": {}", val, e))?;
                }
            }

            if let Some(val) = section.get("zram-fraction") {
                dev.zram_fraction = val.parse()
                    .map_err(|e| format_err!("Failed to parse zram-fraction \"{}\": {}", val, e))?;
            }

            println!("Found configuration for {}: memory-limit={}MB zram-fraction={}",
                     dev.name, dev.memory_limit_mb, dev.zram_fraction);

            if memtotal_mb > dev.memory_limit_mb as f64 {
                println!("{}: system has too much memory ({:.1}MB), limit is {}MB, ignoring.",
                         dev.name,
                         memtotal_mb,
                         dev.memory_limit_mb);
                Ok(None)
            } else {
                dev.disksize = (dev.zram_fraction * memtotal_mb) as u64 * 1024 * 1024;
                Ok(Some(dev))
            }
        }).map(Result::transpose).flatten())
    }

    pub fn run(self) -> Result<(), Error> {
        match self.module {
            ModuleConfig::Generator { output_directory } => run_generator(self.root, self.devices, output_directory),
            ModuleConfig::DeviceSetup { name } => unimplemented!("setting up for {}", name),
        }
    }
}


fn get_total_memory_kb(root: &str) -> Result<u64, Error> {
    let path = Path::new(root).join("proc/meminfo");

    for line in BufReader::new(fs::File::open(&path).with_path(&path)?).lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        if let Some("MemTotal:") = fields.next() {
            if let Some(v) = fields.next() {
                return Ok(v.parse()?);
            }
        }
    }

    Err(format_err!("Couldn't find MemTotal in {}", path.display()))
}
