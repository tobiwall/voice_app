use dotenvy::dotenv;
use portaudio as pa;
use serde_json::Value;
use shuttle_runtime::Error as ShuttleError;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::process::Command;
use std::sync::{Arc, Mutex};
use warp::Filter;
use std::env;

const SAMPLE_RATE: f64 = 44_100.0;
const FRAMES_PER_BUFFER: u32 = 64;

#[shuttle_runtime::main]
async fn shuttle_main() -> Result<MyService, ShuttleError> {
    Ok(MyService {})
}

struct MyService {}

#[shuttle_runtime::async_trait]
impl shuttle_runtime::Service for MyService {
    async fn bind(self, _addr: std::net::SocketAddr) -> Result<(), ShuttleError> {
        // Lade Umgebungsvariablen
        dotenv().ok();
        let api_key = Arc::new(env::var("API_KEY").expect("API_KEY not set"));
        let upload_url = Arc::new(env::var("UPLOAD_URL").expect("UPLOAD_URL not set"));
        let transcript_url = Arc::new(env::var("TRANSCRIPT_URL").expect("TRANSCRIPT_URL not set"));

        // Gemeinsamer Zustand
        let samples = Arc::new(Mutex::new(Vec::new()));
        let is_recording = Arc::new(Mutex::new(false));
        let samples_clone = Arc::clone(&samples);
        let is_recording_clone = Arc::clone(&is_recording);

        // Statische Dateien (Frontend)
        let frontend = warp::fs::dir("./frontend");

        // Route zum Starten der Aufnahme
        let start = warp::path("record")
            .and(warp::post())
            .map({
                let samples = Arc::clone(&samples_clone);
                let is_recording = Arc::clone(&is_recording_clone);

                move || {
                    let mut is_recording_lock = is_recording.lock().unwrap();
                    if !*is_recording_lock {
                        *is_recording_lock = true;
                        let samples = Arc::clone(&samples);
                        tokio::spawn(async move {
                            let pa = pa::PortAudio::new().unwrap();
                            let input_params: pa::InputStreamSettings<f32> = pa
                                .default_input_stream_settings(1, SAMPLE_RATE, FRAMES_PER_BUFFER)
                                .unwrap();
                            let mut stream = pa
                                .open_non_blocking_stream(
                                    input_params,
                                    move |pa::InputStreamCallbackArgs { buffer, .. }| {
                                        let mut samples_lock = samples.lock().unwrap();
                                        samples_lock.extend_from_slice(buffer);
                                        pa::Continue
                                    },
                                )
                                .unwrap();
                            stream.start().unwrap();
                            println!("Recording started.");
                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await; // Aufnahmezeit
                            stream.stop().unwrap();
                            stream.close().unwrap();
                            println!("Recording stopped.");
                        });
                    }
                    warp::reply::with_status("Recording started", warp::http::StatusCode::OK)
                }
            });

        // Route zum Stoppen der Aufnahme
        let stop = warp::path("stop_recording")
            .and(warp::post())
            .map({
                let samples = Arc::clone(&samples_clone);
                let is_recording = Arc::clone(&is_recording_clone);
                let api_key = Arc::clone(&api_key);
                let upload_url = Arc::clone(&upload_url);
                let transcript_url = Arc::clone(&transcript_url);

                move || {
                    let mut is_recording_lock = is_recording.lock().unwrap();
                    if *is_recording_lock {
                        *is_recording_lock = false;
                        println!("Recording stopped.");

                        let samples_lock = samples.lock().unwrap();
                        let file_path = "recording.wav";
                        match save_samples_to_file(&samples_lock, file_path) {
                            Ok(_) => {
                                println!("Audio saved to file: {}", file_path);
                                let api_key = Arc::clone(&api_key);
                                let upload_url = Arc::clone(&upload_url);
                                let transcript_url = Arc::clone(&transcript_url);
                                tokio::spawn(async move {
                                    match upload_and_transcribe(file_path, &api_key, &upload_url, &transcript_url).await {
                                        Ok(transcription) => {
                                            println!("Transcription: {}", transcription)
                                        }
                                        Err(e) => eprintln!("Error during transcription: {}", e),
                                    }
                                });
                            }
                            Err(e) => eprintln!("Error saving audio file: {}", e),
                        }
                    }
                    warp::reply::with_status("Recording stopped", warp::http::StatusCode::OK)
                }
            });

        // Kombiniere Routen
        let routes = frontend.or(start).or(stop);

        println!("Starting Warp server...");
        warp::serve(routes).run(([0, 0, 0, 0], 8080)).await;
        println!("Warp server has stopped.");

        Ok(())
    }
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

async fn upload_and_transcribe(file_path: &str, api_key: &Arc<String>, upload_url: &Arc<String>, transcript_url: &Arc<String>) -> Result<String, Box<dyn Error>> {
    let client = reqwest::Client::new();

    let mut file = File::open(file_path)?;
    let mut audio_data = Vec::new();
    file.read_to_end(&mut audio_data)?;

    let upload_response = client
        .post(upload_url.as_str())
        .header("authorization", api_key.as_str())
        .header("content-type", "audio/wav")
        .body(audio_data)
        .send()
        .await?
        .json::<Value>()
        .await?;

    let audio_url = upload_response["upload_url"]
        .as_str()
        .ok_or("Failed to get upload URL")?;

    let transcript_request = client
        .post(transcript_url.as_str())
        .header("authorization", api_key.as_str())
        .json(&serde_json::json!({ "audio_url": audio_url }))
        .send()
        .await?
        .json::<Value>()
        .await?;

    let transcript_id = transcript_request["id"]
        .as_str()
        .ok_or("Failed to get transcript ID")?;

    loop {
        let status_response = client
            .get(format!("{}/{}", transcript_url.as_str(), transcript_id))
            .header("authorization", api_key.as_str())
            .send()
            .await?
            .json::<Value>()
            .await?;

        let status = status_response["status"].as_str().unwrap_or("");
        if status == "completed" {
            let transcript_text = status_response["text"].as_str().unwrap_or("");
            handle_transcript(transcript_text);
            return Ok(transcript_text.to_string());
        } else if status == "failed" {
            let error_message = status_response["error"].as_str().unwrap_or("Unknown error");
            return Err(format!("Transcription failed: {}", error_message).into());
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
