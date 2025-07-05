use std::time::Duration;

use clap::{Args as ClapArgs, Parser, Subcommand};
use rand::Rng;

const OPCODE_ACK: u8 = 0xBA;
const OPCODE_NACK: u8 = 0xAA;
const RX_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_TX_RETRIES: u8 = 3;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Calibrate the measured temperature using a reference value
    ///
    /// For testing purposes, a transmission error can be simulated using the '--fakeissue'
    /// option. If no error probability is passed, a default value of 50% is used.
    ///
    /// Error simulation example:
    ///
    /// bikecmd tcalibrate --port COM3 --fakeissue=75 205
    ///
    /// The above command sends temperature calibration data for a current temperature of 20.5 C°,
    /// with the probability of a simulated error occurring set to 75%.
    ///
    /// Note that the '--fakeissue' option has no effect on real errors occurring :)
    ///
    #[command(name = "tcalibrate")]
    TemperatureCalibrate {
        #[command(flatten)]
        args: TemperatureCalibrateArgs,
    },

    /// List all avaliable serial ports
    #[command(name = "list")]
    ListDevices,
}

#[derive(ClapArgs, Clone, Debug)]
struct SerialArgs {
    /// The serial port to use
    #[arg(short = 'p', long = "port")]
    port: String,

    /// The baud rate to use
    #[arg(short = 'r', long = "rate", default_value = "115200")]
    rate: u32,
}

#[derive(ClapArgs, Clone, Debug)]
struct TemperatureCalibrateArgs {
    /// The current temperature (in °C/10) to be used as reference
    #[arg()]
    current_temperature: i16,

    /// Fakes a transmit error by sending an incorrect crc. The probability (0-100) can be specified.
    #[arg(
        long,
        value_parser = clap::value_parser!(u8).range(0..=100),
        default_missing_value="50",
        value_name="PROBABILITY",
        num_args = 0..=1,
        require_equals = true)]
    fakeissue: Option<u8>,

    #[command(flatten)]
    serial: SerialArgs,
}

#[derive(thiserror::Error, Debug)]
enum BikecmdError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Serial(#[from] serialport::Error),

    #[error("Protocol Error: {0}")]
    BikecomputerProto(String),

    #[error("Gave up after {0} retries")]
    RetryStalled(u8),
}

fn main() {
    let args = Args::parse();

    let result = match args.command {
        Command::ListDevices => list_devices(),
        Command::TemperatureCalibrate { args } => temp_calibrate(args),
    };

    match result {
        Ok(_) => (),
        Err(e) => println!("Exiting: {}", e),
    }
}

//list subcommand
fn list_devices() -> Result<(), BikecmdError> {
    let ports = serialport::available_ports()?;

    println!("Available ports (may or may not be a Bike Computer)");
    for port in ports {
        println!(">  {}", port.port_name);
    }
    Ok(())
}

//tcalibrate subcommand
fn temp_calibrate(args: TemperatureCalibrateArgs) -> Result<(), BikecmdError> {
    let mut retries = MAX_TX_RETRIES;
    loop {
        println!("Sending calibration data...");
        match temp_calibrate_run(&args) {
            Ok(_) => {
                println!("Success");
                return Ok(());
            }
            Err(e) => match &e {
                //do not retry on permission problems
                BikecmdError::Serial(inner) if inner.kind == serialport::ErrorKind::NoDevice => return Err(e),
                e => {
                    retries -= 1;
                    println!("An error occurred: {}", e);
                    if retries == 0 {
                        return Err(BikecmdError::RetryStalled(MAX_TX_RETRIES));
                    }
                    println!("Retrying {} more time(s)\n", retries);
                }
            },
        }
    }
}

fn temp_calibrate_run(args: &TemperatureCalibrateArgs) -> Result<(), BikecmdError> {
    let mut port = serialport::new(&args.serial.port, args.serial.rate)
        .timeout(RX_TIMEOUT)
        .open()?;

    // temperature as bytes (little endian)
    let mut calibration_bytes = args.current_temperature.to_le_bytes().to_vec();

    //frame start: 0xFF frame start indicator, 0x02 payload length
    let mut message: Vec<u8> = vec![0xFF, 0x02];
    message.append(&mut calibration_bytes);

    //checksum only includes length and payload
    //the impl on the microcontroller uses the "XMODEM" variant of crc16
    let mut checksum = crc16::State::<crc16::XMODEM>::calculate(&message[1..4]);

    //decide if we want an error
    let fakeissue = match args.fakeissue {
        None => false,
        Some(probability) => {
            let mut rng = rand::rng();
            let random_value = rng.random::<u8>() % 100;
            random_value < probability
        }
    };

    if fakeissue {
        //change checksum if we want to see an error
        checksum += 10;
    }

    //checksum bytes (also little endian)
    let mut checksum = checksum.to_le_bytes().to_vec();
    message.append(&mut checksum);

    port.write_all(&message)?;

    //receive answer, we usually only expect one byte, but if anything very bad happens, we can see
    //it in the bigger buffer
    let mut buf = [0; 100];
    let read_bytes = port.read(&mut buf)?;
    let received = buf[0..read_bytes].to_vec();

    if read_bytes != 1 {
        return Err(BikecmdError::BikecomputerProto(format!(
            "Expected 1 ACK/NACK byte, received {} bytes instead",
            read_bytes
        )));
    }

    match received[0] {
        OPCODE_ACK => Ok(()),
        //TODO retry sending another time on NACK
        OPCODE_NACK => Err(BikecmdError::BikecomputerProto("Received NACK".to_string())),
        //catch-all
        symbol => Err(BikecmdError::BikecomputerProto(format!(
            "ACK not received, received {:#04x} instead",
            symbol
        ))),
    }
}
