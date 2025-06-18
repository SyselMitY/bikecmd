use std::time::Duration;

use clap::{Parser, Subcommand};

const OPCODE_ACK: u8 = 0xBA;
const OPCODE_NACK: u8 = 0xAA;
const RX_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Calibrate the measured temperature using a reference value
    #[command(name = "tcalibrate")]
    TemperatureCalibrate {
        /// The serial port to use
        #[arg(short = 'p', long = "port")]
        serial_port: String,

        /// The baud rate to use
        #[arg(short = 'r', long = "rate", default_value = "115200")]
        rate: u32,

        /// The current temperature (in °C/10) to be used as reference
        #[arg()]
        current_temperature: i16,

        /// Fakes a transmit error by sending an incorrect crc
        #[arg(long)]
        fakeissue: bool,
    },

    /// List all avaliable serial ports
    #[command(name = "list")]
    ListDevices,
}

#[derive(thiserror::Error, Debug)]
enum BikecmdError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Serial(#[from] serialport::Error),

    #[error("Protocol Error: {0}")]
    BikecomputerProto(String),
}

fn main() {
    let args = Args::parse();

    let result = match &args.command {
        Command::ListDevices => list_devices(),
        Command::TemperatureCalibrate {
            serial_port,
            rate,
            current_temperature,
            fakeissue,
        } => temp_calibrate(serial_port, *rate, *current_temperature, *fakeissue),
    };

    match result {
        Ok(_) => (),
        Err(e) => print!("An error occurred: {}", e),
    }
}

fn list_devices() -> Result<(), BikecmdError> {
    let ports = serialport::available_ports()?;

    println!("Available ports (may or may not be a Bike Computer)");
    for port in ports {
        println!(">  {}", port.port_name);
    }
    Ok(())
}

fn temp_calibrate(
    port: &String,
    rate: u32,
    temp: i16,
    fakeissue: bool,
) -> Result<(), BikecmdError> {
    let mut port = serialport::new(port, rate)
        .timeout(RX_TIMEOUT)
        .open()?;

    // temperature as bytes (little endian)
    let mut calibration_bytes = temp.to_le_bytes().to_vec();

    //frame start: 0xFF frame start indicator, 0x02 payload length
    let mut message: Vec<u8> = vec![0xFF, 0x02];
    message.append(&mut calibration_bytes);

    //checksum only includes length and payload
    //the impl on the microcontroller uses the "XMODEM" variant of crc16
    let mut checksum = crc16::State::<crc16::XMODEM>::calculate(&message[1..4]);
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
        OPCODE_NACK => Err(BikecmdError::BikecomputerProto(
            "Received NACK, please retry".to_string(),
        )),
        //catch-all
        symbol => Err(BikecmdError::BikecomputerProto(format!(
            "ACK not received, received {:#04x} instead",
            symbol
        ))),
    }
}
