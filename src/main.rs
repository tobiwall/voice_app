use portaudio as pa;
use serde_json::Value;
use shuttle_runtime::Error as ShuttleError;
use std::error::Error;
use std::fs::File;
use std::io::{Read};
use std::process::Command;
use std::sync::{Arc, Mutex};
use warp::http::Response;
use warp::Filter;

const SAMPLE_RATE: f64 = 44_100.0;
const FRAMES_PER_BUFFER: u32 = 64;
const API_KEY: &str = "b4e9064be98642d6bc4d1216dcea51ce";
const UPLOAD_URL: &str = "https://api.assemblyai.com/v2/upload";
const TRANSCRIPT_URL: &str = "https://api.assemblyai.com/v2/transcript";

#[shuttle_runtime::main]
async fn shuttle_main() -> Result<MyService, ShuttleError> {
    Ok(MyService {})
}

struct MyService {}

#[shuttle_runtime::async_trait]
impl shuttle_runtime::Service for MyService {
    async fn bind(self, _addr: std::net::SocketAddr) -> Result<(), ShuttleError> {
        // Start your service and bind to the socket address
        tokio::spawn(async move {
            // Set up shared state
            let samples = Arc::new(Mutex::new(Vec::new()));
            let is_recording = Arc::new(Mutex::new(false));
            let samples_clone = Arc::clone(&samples);
            let is_recording_clone = Arc::clone(&is_recording);

            // Start recording route
            let start = warp::path("record").and(warp::post()).map({
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
                                    move |pa::InputStreamCallbackArgs {
                                              buffer,  ..
                                          }| {
                                        let mut samples_lock = samples.lock().unwrap();
                                        samples_lock.extend_from_slice(buffer);
                                        pa::Continue
                                    },
                                )
                                .unwrap();
                            stream.start().unwrap();
                            println!("Recording started.");
                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await; // Beispiel: Aufnahme für 10 Sekunden
                            stream.stop().unwrap();
                            stream.close().unwrap();
                            println!("Recording stopped.");
                        });
                    }
                    Response::builder()
                        .header("Access-Control-Allow-Origin", "*")
                        .header("Access-Control-Allow-Methods", "POST, GET, OPTIONS")
                        .header("Access-Control-Allow-Headers", "Content-Type")
                        .status(200)
                        .body("Recording started")
                        .unwrap()
                }
            });

            // Stop recording route
            let stop = warp::path("stop_recording").and(warp::post()).map({
                let samples = Arc::clone(&samples_clone);
                let is_recording = Arc::clone(&is_recording_clone);
                move || {
                    let mut is_recording_lock = is_recording.lock().unwrap();
                    if *is_recording_lock {
                        *is_recording_lock = false;
                        println!("Recording stopped.");

                        // Save samples to file and trigger transcription process
                        let samples_lock = samples.lock().unwrap();
                        let file_path = "recording.wav";
                        match save_samples_to_file(&samples_lock, file_path) {
                            Ok(_) => {
                                println!("Audio saved to file: {}", file_path);
                                // Now upload and transcribe the file
                                tokio::spawn(async move {
                                    match upload_and_transcribe(file_path).await {
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
                    Response::builder()
                        .header("Access-Control-Allow-Origin", "*")
                        .header("Access-Control-Allow-Methods", "POST, GET, OPTIONS")
                        .header("Access-Control-Allow-Headers", "Content-Type")
                        .status(200)
                        .body("Recording stopped")
                        .unwrap()
                }
            });

            // Handle OPTIONS requests for CORS preflight checks
            let options = warp::options().map(|| {
                warp::reply::with_header(warp::reply(), "Access-Control-Allow-Origin", "*")
            });

            // Combine the routes
            let routes = start.or(stop).or(options);

            // Run the server
            warp::serve(routes).run(_addr).await;
        });

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

async fn upload_and_transcribe(file_path: &str) -> Result<String, Box<dyn Error>> {
    let client = reqwest::Client::new();

    let mut file = File::open(file_path)?;
    let mut audio_data = Vec::new();
    file.read_to_end(&mut audio_data)?;

    let upload_response = client
        .post(UPLOAD_URL)
        .header("authorization", API_KEY)
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
        .post(TRANSCRIPT_URL)
        .header("authorization", API_KEY)
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
            .get(format!("{}/{}", TRANSCRIPT_URL, transcript_id))
            .header("authorization", API_KEY)
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
