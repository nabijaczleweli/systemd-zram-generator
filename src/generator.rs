/* SPDX-License-Identifier: MIT */

use crate::config::Device;
use crate::ResultExt;
use failure::Error;
use std::borrow::Cow;
use std::fs;
use std::io::{prelude::*, BufReader};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};


fn make_parent(of: &Path) -> Result<(), Error> {
    let parent = of.parent()
        .ok_or_else(|| format_err!("Couldn't get parent of {}", of.display()))?;
    fs::create_dir_all(&parent)?;
    Ok(())
}

fn make_symlink(dst: &str, src: &Path) -> Result<(), Error> {
    make_parent(src)?;
    symlink(dst, src).with_path(src)?;
    Ok(())
}

fn virtualization_container() -> Result<bool, Error> {
    match Command::new("systemd-detect-virt").arg("--container").stdout(Stdio::null()).status() {
        Ok(status) => Ok(status.success()),
        Err(e) => Err(format_err!("systemd-detect-virt call failed: {}", e)),
    }
}


pub fn run_generator(root: Cow<'static, str>, devices: Vec<Device>, output_directory: PathBuf) -> Result<(), Error> {
    let memtotal = get_total_memory_kb(&root)?;
    let memtotal_mb = memtotal as f64 / 1024.;

    if virtualization_container()? {
        println!("Running in a container, exiting.");
        return Ok(());
    }

    let mut devices_made = false;
    for dev in &devices {
        devices_made |= handle_device(&root, &output_directory, dev, memtotal_mb)?;
    }
    if devices_made {
        /* We created some services, let's make sure the module is loaded */
        let modules_load_path = Path::new(&root[..]).join("run/modules-load.d/zram.conf");
        make_parent(&modules_load_path)?;
        fs::write(&modules_load_path, "zram\n").with_path(modules_load_path)?;
    }

    Ok(())
}

fn handle_device(root: &str, output_directory: &Path, device: &Device, memtotal_mb: f64) -> Result<bool, Error> {
    if memtotal_mb > device.memory_limit_mb as f64 {
        println!("{}: system has too much memory ({:.1}MB), limit is {}MB, ignoring.",
                 device.name,
                 memtotal_mb,
                 device.memory_limit_mb);
        return Ok(false);
    }

    let disksize = (device.zram_fraction * memtotal_mb) as u64 * 1024 * 1024;
    let service_name = format!("swap-create@{}.service", device.name);
    println!("Creating {} for {}dev/{} ({}MB)",
             service_name, root, device.name, disksize / 1024 / 1024);

    let service_path = output_directory.join(&service_name);

    let contents = format!("\
[Unit]
Description=Create swap on {root}dev/%i
Wants=systemd-modules-load.service
After=systemd-modules-load.service
After={device_name}
DefaultDependencies=false

[Service]
Type=oneshot
ExecStartPre=-modprobe zram
ExecStart=sh -c 'echo {disksize} >{root}sys/block/%i/disksize'
ExecStart=mkswap {root}dev/%i
",
        root = root,
        device_name = format!("dev-{}.device", device.name),
        disksize = disksize,
    );
    fs::write(&service_path, contents).with_path(service_path)?;

    let swap_name = format!("dev-{}.swap", device.name);
    let swap_path = output_directory.join(&swap_name);

    let contents = format!("\
[Unit]
Description=Compressed swap on {root}dev/{zram_device}
Requires={service}
After={service}

[Swap]
What={root}dev/{zram_device}
Options=pri=100
",
        root = root,
        service = service_name,
        zram_device = device.name
    );

    fs::write(&swap_path, contents).with_path(swap_path)?;

    let symlink_path = output_directory.join("swap.target.wants").join(&swap_name);
    let target_path = format!("../{}", swap_name);
    make_symlink(&target_path, &symlink_path)?;
    Ok(true)
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
