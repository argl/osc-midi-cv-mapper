#![allow(clippy::collapsible_match)]
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, StreamConfig};
use midir::{MidiOutput, MidiOutputConnection};
use rosc::{OscMessage, OscPacket, decoder};
use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value = "8000")]
    osc_port: u16,

    #[arg(long)]
    audio_device: Option<String>,

    #[arg(long)]
    midi_device: Option<String>,
}

fn find_audio_device(name: &Option<String>) -> Device {
    let host = cpal::default_host();
    println!("Available audio devices:");
    for device in host.output_devices().unwrap() {
        println!(
            "{}: {}",
            device.name().unwrap(),
            device.default_output_config().unwrap().sample_format()
        );
    }
    if let Some(name) = name {
        host.output_devices()
            .unwrap()
            .find(|d| d.name().unwrap().contains(name))
            .expect("Audio device not found")
    } else {
        host.default_output_device()
            .expect("No default audio device")
    }
}

fn find_midi_device(name: &Option<String>) -> MidiOutputConnection {
    let midi_out = MidiOutput::new("OSC-MIDI-Bridge").unwrap();
    let ports = midi_out.ports();

    println!("Available MIDI devices:");
    for (i, port) in ports.iter().enumerate() {
        println!("{}: {}", i, midi_out.port_name(port).unwrap());
    }

    let port = if let Some(name) = name {
        ports
            .iter()
            .find(|p| midi_out.port_name(p).unwrap().contains(name))
            .expect("MIDI device not found")
    } else {
        &ports[0]
    };

    println!("Using MIDI device: {}", midi_out.port_name(port).unwrap());

    midi_out
        .connect(port, "osc-midi")
        .expect("Failed to connect MIDI device")
}

fn main() {
    let args = Args::parse();

    let audio_device = find_audio_device(&args.audio_device);
    println!("Using audio device: {}", audio_device.name().unwrap());
    let midi_conn = Arc::new(Mutex::new(find_midi_device(&args.midi_device)));

    let channels = 8;
    let latest_values = Arc::new(Mutex::new(vec![0f32; channels]));

    let config = StreamConfig {
        channels: channels as u16,
        sample_rate: SampleRate(48000),
        buffer_size: cpal::BufferSize::Default,
    };

    let values_clone = latest_values.clone();

    let stream = audio_device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                let values = values_clone.lock().unwrap();
                for frame in data.chunks_mut(channels) {
                    for (sample, val) in frame.iter_mut().zip(values.iter()) {
                        *sample = *val;
                    }
                }
            },
            move |err| eprintln!("Audio error: {}", err),
            None,
        )
        .unwrap();

    stream.play().unwrap();

    let osc_socket = UdpSocket::bind(format!("0.0.0.0:{}", args.osc_port)).unwrap();
    println!("Listening on OSC port {}", args.osc_port);

    let osc_address_map: HashMap<&str, usize> = [
        ("/lfo1", 2),
        ("/lfo2", 3),
        ("/lfo3", 4),
        ("/lfo4", 5),
        ("/stepped32", 6),
        ("/stepped8", 7),
    ]
    .iter()
    .cloned()
    .collect();

    let mut buf = [0u8; 1024];
    loop {
        if let Ok((size, _)) = osc_socket.recv_from(&mut buf) {
            if let Ok((_, packet)) = decoder::decode_udp(&buf[..size]) {
                if let OscPacket::Message(OscMessage { addr, args, .. }) = packet {
                    if let Some(&channel) = osc_address_map.get(addr.as_str()) {
                        if let Some(rosc::OscType::Float(value)) = args.first() {
                            let audio_val = value * 2.0 - 1.0;
                            let midi_val = (value * 127.0).clamp(0.0, 127.0) as u8;

                            {
                                let mut vals = latest_values.lock().unwrap();
                                vals[channel] = audio_val;
                            }

                            let midi_message = [0xB0, channel as u8, midi_val];
                            midi_conn.lock().unwrap().send(&midi_message).unwrap();

                            println!(
                                "{} -> Channel {}: Audio {}, MIDI {}",
                                addr,
                                channel + 1,
                                audio_val,
                                midi_val
                            );
                        }
                    }
                }
            }
        }
    }
}
