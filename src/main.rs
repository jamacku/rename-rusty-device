extern crate syslog;
#[macro_use]
extern crate log;

use std::env;
use std::error;
use std::path::Path;
use std::fs::File;
use std::io:: {
    prelude::*,
    BufReader
};

use mac_address:: {
    mac_address_by_name,
    MacAddress
};

use glob::glob_with;

use lazy_static::lazy_static;
use regex::Regex;

use syslog:: {
    Facility,
    Formatter3164,
    BasicLogger
};

use log::LevelFilter;


// --- --- --- //

/* Implement conversion from any type that implements the Error trait into the trait object Box<Error>
 * https://doc.rust-lang.org/std/keyword.dyn.html */
type Result<T> = std::result::Result<T, Box<dyn error::Error>>;

const ENV: &str = "INTERFACE";
const CONFIG_DIR: &str = "/etc/sysconfig/network-scripts";
const KERNEL_CMDLINE: &str = "/proc/cmdline";


// --- --- --- //

fn main() -> Result<()> {
    /* Setup syslog logger */ 
    let formatter = Formatter3164 {
        facility: Facility::LOG_USER,
        hostname: None,
        process: "ifcfg_devname".into(),
        pid: 0,
    };

    let logger = syslog::unix(formatter).expect("[ifcfg_devname]: could not connect to syslog");
    /* This is a simple convenience wrapper over set_logger */
    log::set_boxed_logger(Box::new(BasicLogger::new(logger)))
        .map(|()| log::set_max_level(LevelFilter::Info))?;

    debug!("Connected to syslog");

    /* Read env variable INTERFACE in order to get names of if */
    let kernel_if_name = match env::var_os(ENV).unwrap().into_string() {
        Ok(val) => val,
        Err(_err) => {
            warn!("Error while processing ENV INTERFACE");
            std::process::exit(1)
        }
    };

    /* Get MAC address of given interface */
    let mac_address = match mac_address_by_name(&kernel_if_name) {
        Ok(Some(val)) => val,
        _ => {
            warn!("Error while getting MAC address of given network interface ({})", kernel_if_name);
            std::process::exit(1)
        }
    }; 
    
    /* Let's check kernel cmdline and also process ifname= entries
     * as they are documented in dracut.cmdline(7)
     * Example: ifname=test:aa:bb:cc:dd:ee:ff
     */
    let mut device_config_name = match parse_kernel_cmdline(&mac_address) {
        Ok(Some(name)) => name,
        _ => {
            debug!("New device name for '{}' wasn't found at kernel cmdline", kernel_if_name);
            String::from("")
        }
    };

    /* When device was not found at kernel cmdline look into ifcfg files */
    if device_config_name.is_empty() {
        /* Scan config dir and look for ifcfg-* files */
        let config_dir = Path::new(CONFIG_DIR);
        let list_of_ifcfg_paths = match scan_config_dir(config_dir) {
            Some(val) => val,
            None => {
                warn!("Error while getting list of ifcfg files from directory /etc/sysconfig/network-scripts/");
                std::process::exit(1)
            }
        };

        /* Loop through ifcfg configurations and look for matching MAC address and return DEVICE name */
        device_config_name = String::new();
        'config_loop: for path in list_of_ifcfg_paths {
            let config_file_path: &Path = Path::new(&path);

            match parse_config_file(config_file_path, &mac_address) {
                Ok(Some(name)) => {
                    device_config_name = format!("{}", name);
                    break 'config_loop;
                }
                _ => continue
            }
        }
    }

    if !device_config_name.is_empty() {
        println!("{}", device_config_name);
        Ok(())
    } else {
        warn!("Device name or MAC address weren't found in ifcfg files.");
        std::process::exit(1);
    }
}


// --- Functions --- //
/* Scan directory /etc/sysconfig/network-scripts for ifcfg files */
fn scan_config_dir(config_dir: &Path) -> Option<Vec<String>> {
    let glob_options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };

    let glob_pattern = config_dir.to_str()?.to_owned() + "/ifcfg-*";

    let mut list_of_config_paths = vec![];

    for entry in glob_with(&glob_pattern, glob_options).unwrap() {
        match entry {
            Ok(path) => {
                list_of_config_paths.push(path.to_str()?.to_owned());
            },
            Err(_err) => continue
        };
    }

    if !list_of_config_paths.is_empty() {
        Some(list_of_config_paths)
    } else {
        None
    }
}

/* Scan ifcfg files and look for given HWADDR and return DEVICE name */
fn parse_config_file(config_file: &Path, mac_address: &MacAddress) -> Result<Option<String>> {
    let file = File::open(config_file)?;
    let reader = BufReader::new(file);
    let mut hwaddr: Option<MacAddress> = None;
    let mut device: Option<String> = None;

    lazy_static! {
        /* Look for line that starts with DEVICE= and then store everything else in group
         * regex: ^DEVICE=(\S[^:]{1,15})
         * ^DEVICE=(group1) - look for line starting with `^DEVICE=` following with group of characters describing new device name
         * group1: (\S[^:]{1,15}) - match non-whitespace characters ; minimum 1 and maximum 15 ; do not match `:` character
         * example: DEVICE=new-devname007
         *                 ^^^^^^^^^^^^^^
         *                 new dev name */
        static ref REGEX_DEVICE: Regex = Regex::new(r"^DEVICE=(\S[^:]{1,15})").unwrap();

        /* Look for line with mac address and store its value in group for later
         * regex: ^HWADDR=(([0-9A-Fa-f]{2}[:]){5}([0-9A-Fa-f]{2}))
         * ^HWADDR=(group1) - look for line starting with `^HWADDR=` following with group of characters describing hw address of device
         * group1: (([0-9A-Fa-f]{2}[:]){5}([0-9A-Fa-f]{2})) - match 48-bit hw address expressed in hexadecimal system ; each of inner 8-bits are separated with `:` character ; case insensitive
         * example: HWADDR=00:1b:44:11:3A:B7
         *                 ^^^^^^^^^^^^^^^^^
         *                 hw address of if */
        static ref REGEX_HWADDR: Regex = Regex::new(r"^HWADDR=(([0-9A-Fa-f]{2}[:]){5}([0-9A-Fa-f]{2}))").unwrap();
    }

    /* Read lines of given file and look for DEVICE= and HWADDR= */
    for line in reader.lines() {
        let line = line?;

        /* Look for HWADDR= */
        if REGEX_HWADDR.is_match(&line) {
            for capture in REGEX_HWADDR.captures_iter(&line) {
                hwaddr = Some(capture[1].parse()?);
            }
        }

        /* Look for DEVICE= */
        if REGEX_DEVICE.is_match(&line) {
            for capture in REGEX_DEVICE.captures_iter(&line) {
                device = Some(capture[1].parse()?);
            }
        }
    }

    if hwaddr.is_some() {
        if hwaddr
            .unwrap()
            .to_string()
            .to_owned()
            .to_lowercase()
            .ne(
                &mac_address
                    .to_string()
                    .to_owned()
                    .to_lowercase()
        ) {
            device = None;
        }
    }

    /* When MAC doesn't match it returns OK(None) */
    match device {
        dev => Ok(dev)
    }
}

/* Scan kernel cmdline and look for given hardware address and return new device name */
#[allow(unused)]
fn parse_kernel_cmdline(mac_address: &MacAddress) -> Result<Option<String>> {
    let file = File::open(KERNEL_CMDLINE).unwrap();
    let mut reader = BufReader::new(file);
    let mut hwaddr: Option<MacAddress> = None;
    let mut device: Option<String> = None;
    let mut kernel_cmdline = String::new();

    lazy_static! {
        /* Look for patterns like this ifname=new_name:aa:BB:CC:DD:ee:ff at kernel command line
         * regex: ifname=(\S[^:]{1,15}):(([0-9A-Fa-f]{2}[:-]){5}([0-9A-Fa-f]{2}))
         * ifname=(group1):(group2) - look for pattern starting with `ifname=` following with two groups separated with `:` character
         * group1: (\S[^:]{1,15}) - match non-whitespace characters ; minimum 1 and maximum 15 ; do not match `:` character
         * group2: (([0-9A-Fa-f]{2}[:]){5}([0-9A-Fa-f]{2})) - match 48-bit hw address expressed in hexadecimal system ; each of inner 8-bits are separated with `:` character ; case insensitive
         * example: ifname=new-devname007:00:1b:44:11:3A:B7
         *                 ^^^^^^^^^^^^^^ ~~~~~~~~~~~~~~~~~
         *                 new dev name   hw address of if */
        static ref REGEX_DEVICE_HWADDR_PAIR: Regex = Regex::new(r"ifname=(\S[^:]{1,15}):(([0-9A-Fa-f]{2}[:]){5}([0-9A-Fa-f]{2}))").unwrap();
    }

    /* Read kernel command line and look for ifname= */
    reader.read_line(&mut kernel_cmdline)?;

    /* Look for ifname= */
    if REGEX_DEVICE_HWADDR_PAIR.is_match(&kernel_cmdline) {
        for capture in REGEX_DEVICE_HWADDR_PAIR.captures_iter(&kernel_cmdline) {
            device = Some(capture[1].parse()?);
            hwaddr = Some(capture[2].parse()?);
                
            /* Check MAC */
            if hwaddr.is_some() {
                if hwaddr
                    .unwrap()
                    .to_string()
                    .to_owned()
                    .to_lowercase()
                    .eq(
                        &mac_address
                            .to_string()
                            .to_owned()
                            .to_lowercase()
                ) {
                    break;
                } else {
                    device = None;
                }
            }
        }
    }

    /* When MAC doesn't match it returns OK(None) */
    match device {
        dev => Ok(dev)
    }
}
