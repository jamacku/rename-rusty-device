#[macro_use]
extern crate log;

use std::env;
use std::error;
use std::path::Path;
use std::str::FromStr;

use mac_address::{mac_address_by_name, MacAddress};

mod lib;
mod logger;
mod parse;
mod scan;

const ENV: &str = "INTERFACE";
const CONFIG_DIR: &str = "/etc/sysconfig/network-scripts";
const KERNEL_CMDLINE: &str = "/proc/cmdline";

fn main() -> Result<(), Box<dyn error::Error>> {
    //let input = lib::process_arguments(&env::args());
    let args: Vec<String> = env::args().collect();
    let is_correct_number_args = args.len() > 3;

    logger::init();

    let kernel_interface_name = match env::var_os(ENV).unwrap().into_string() {
        Ok(val) => val,
        Err(err) => {
            error!("Fail obtaining ENV {} - {}", ENV, err.to_string_lossy());
            std::process::exit(1)
        }
    };

    /* Check for testing hw address passed via arg */
    let mac_address = if !is_correct_number_args {
        /* Get MAC address of given interface */
        match mac_address_by_name(&kernel_interface_name) {
            Ok(Some(val)) => val,
            _ => {
                error!("Fail to resolve MAC address of '{}'", kernel_interface_name);
                std::process::exit(1)
            }
        }
    } else {
        MacAddress::from_str(&args[3])?
    };

    let simple_mac_address = mac_address.to_string().to_lowercase();

    /* Check for alternative path to kernel cmdline */
    let kernel_cmdline = if !is_correct_number_args {
        Path::new(KERNEL_CMDLINE)
    } else {
        Path::new(&args[1])
    };

    /* Let's check kernel cmdline and also process ifname= entries
     * as they are documented in dracut.cmdline(7)
     * Example: ifname=test:aa:bb:cc:dd:ee:ff
     */
    let mut device_config_name = match parse::kernel_cmdline(&simple_mac_address, kernel_cmdline) {
        Ok(Some(name)) => {
            if lib::is_like_kernel_name(&name) {
                warn!("Don't use kernel names (eth0, etc.) as new names for network devices! Used name: '{}'", name);
            }
            name
        }
        _ => {
            debug!(
                "New device name for '{}' wasn't found at kernel cmdline",
                kernel_interface_name
            );
            String::from("")
        }
    };

    /* When device was not found at kernel cmdline look into ifcfg files */
    if device_config_name.is_empty() {
        /* Check for alternative path to config dir */
        let config_dir = if !is_correct_number_args {
            CONFIG_DIR
        } else {
            &args[2]
        };

        /* Scan config dir and look for ifcfg-* files */
        let config_dir_path = Path::new(config_dir);
        let ifcfg_paths = match scan::config_dir(config_dir_path) {
            Some(val) => val,
            None => {
                error!(
                    "Fail to get list of ifcfg files from directory {}",
                    config_dir
                );
                std::process::exit(1)
            }
        };

        /* Loop through ifcfg configurations and look for matching MAC address and return DEVICE name */
        device_config_name = String::new();
        for path in ifcfg_paths {
            let config_file_path: &Path = Path::new(&path);

            match parse::config_file(config_file_path, &simple_mac_address) {
                Ok(Some(name)) => {
                    if lib::is_like_kernel_name(&name) {
                        warn!("Don't use kernel names (eth0, etc.) as new names for network devices! Used name: '{}'", name);
                    }
                    device_config_name = format!("{}", name);
                    break;
                }
                _ => continue,
            }
        }
    }

    if !device_config_name.is_empty() {
        println!("{}", device_config_name);
        Ok(())
    } else {
        error!("Device name or MAC address weren't found in ifcfg files.");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod unit_test;
