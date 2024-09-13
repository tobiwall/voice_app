extern crate portaudio;
use hound;
use portaudio as pa;
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use std::fs::File;
use std::io::stdin;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::{Arc, Mutex};

const SAMPLE_RATE: f64 = 44_100.0;
const FRAMES_PER_BUFFER: u32 = 64;
const API_KEY: &str = "b4e9064be98642d6bc4d1216dcea51ce";
const UPLOAD_URL: &str = "https://api.assemblyai.com/v2/upload";
const TRANSCRIPT_URL: &str = "https://api.assemblyai.com/v2/transcript";

#[tokio::main]

async fn main() -> Result<(), Box<dyn Error>> {
    let pa = pa::PortAudio::new()?;
    let default_input_device = pa.default_input_device()?;
    let input_device_info = pa.device_info(default_input_device)?;

    println!("Default input device: {}", input_device_info.name);

    let input_params: pa::InputStreamSettings<f32> =
        pa.default_input_stream_settings(1, SAMPLE_RATE, FRAMES_PER_BUFFER)?;

    let samples = Arc::new(Mutex::new(Vec::new()));
    let samples_clone = Arc::clone(&samples);

    let mut stream = pa.open_non_blocking_stream(
        input_params,
        move |pa::InputStreamCallbackArgs { buffer, frames, .. }| {
            let mut samples_lock = samples_clone.lock().unwrap();
            samples_lock.extend_from_slice(buffer);
            pa::Continue
        },
    )?;

    stream.start()?;
    println!("Recording started. Press Enter to stop...");

    let mut input = String::new();
    stdin().read_line(&mut input).unwrap();

    stream.stop()?;
    stream.close()?;

    let samples_lock = samples.lock().unwrap();
    println!("Number of samples collected: {}", samples_lock.len());

    let audio_file_path = "recorded_audio.wav";
    save_samples_to_file(&samples_lock, audio_file_path)?;

    let transcript = upload_and_transcribe(audio_file_path).await?;
    println!("Transcript: {}", transcript);

    Ok(())
}

fn save_samples_to_file(samples: &[f32], path: &str) -> Result<(), Box<dyn Error>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)?;

    for &sample in samples {
        let int_sample = (sample * i16::MAX as f32) as i16;
        writer.write_sample(int_sample)?;
    }

    writer.finalize()?;
    Ok(())
}

async fn upload_and_transcribe(file_path: &str) -> Result<String, Box<dyn Error>> {
    let client = reqwest::Client::new();

    let mut file = std::fs::File::open(file_path)?;
    let mut audio_data = Vec::new();
    file.read_to_end(&mut audio_data)?;

    let upload_response = client
        .post(UPLOAD_URL)
        .header("authorization", API_KEY)
        .header("content-type", "audio/wav")
        .body(audio_data)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let audio_url = upload_response["upload_url"]
        .as_str()
        .ok_or("Failed to get upload URL")?;

    let transcript_request = client
        .post(TRANSCRIPT_URL)
        .header("authorization", API_KEY)
        .json(&serde_json::json!({ "audio_url": audio_url }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let transcript_id = transcript_request["id"]
        .as_str()
        .ok_or("Failed to get transcript ID")?;

    loop {
        let status_response = client
            .get(format!("{}/{}", TRANSCRIPT_URL, transcript_id))
            .header("authorization", API_KEY)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let status = status_response["status"].as_str().unwrap_or("");
        if status == "completed" {
            let transcript_text = status_response["text"].as_str().unwrap_or("");
            handle_transcript(transcript_text);
            return Ok(transcript_text.to_string());
        } else if status == "failed" {
            return Err("Transcription failed.".into());
        } else {
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        }
    }
}

fn handle_transcript(transcript_text: &str) {
    if transcript_text.contains("weather") {
        println!("Opening weather app...");
        let status = Command::new("open")
            .arg("/System/Applications/Weather.app")
            .status()
            .expect("Failed to open weather app");

        if !status.success() {
            eprintln!("Error opening weather app: {:?}", status);
        }
    } else if transcript_text.contains("calculator") {
        println!("Opening calculator...");
        let status = Command::new("open")
            .arg("/System/Applications/Calculator.app")
            .status()
            .expect("Failed to open calculator");

        if !status.success() {
            eprintln!("Error opening calculator: {:?}", status);
        }
    }
}
