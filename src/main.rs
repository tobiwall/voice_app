use portaudio as pa;
use serde_json::Value;
use shuttle_runtime::Error as ShuttleError;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::process::Command;
use std::sync::{Arc, Mutex};
use warp::Filter;
use dotenvy::dotenv;
use std::env;

const SAMPLE_RATE: f64 = 44_100.0;
const FRAMES_PER_BUFFER: u32 = 64;

#[shuttle_runtime::main]
async fn shuttle_main() -> Result<MyService, ShuttleError> {
    dotenv().ok(); // dotenvy verwenden, um .env Datei zu laden
    Ok(MyService {})
}

struct MyService {}

#[shuttle_runtime::async_trait]
impl shuttle_runtime::Service for MyService {
    async fn bind(self, _addr: std::net::SocketAddr) -> Result<(), ShuttleError> {
        // Set up shared state
        let samples = Arc::new(Mutex::new(Vec::new()));
        let is_recording = Arc::new(Mutex::new(false));
        let samples_clone = Arc::clone(&samples);
        let is_recording_clone = Arc::clone(&is_recording);

        // CORS Filter
        let cors = warp::cors()
            .allow_any_origin()
            .allow_methods(vec!["POST", "GET", "OPTIONS"])
            .allow_headers(vec!["Content-Type"]);

        // Start recording route
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
                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await; // Beispiel: Aufnahme für 10 Sekunden
                            stream.stop().unwrap();
                            stream.close().unwrap();
                            println!("Recording stopped.");
                        });
                    }
                    warp::reply::with_status("Recording started", warp::http::StatusCode::OK)
                }
            });

        // Stop recording route
        let stop = warp::path("stop_recording")
            .and(warp::post())
            .map({
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
                    warp::reply::with_status("Recording stopped", warp::http::StatusCode::OK)
                }
            });

        // Combine the routes with CORS
        let routes = start.or(stop).with(cors);

        println!("Starting Warp server...");
        // Run the server directly with a different port
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

async fn upload_and_transcribe(file_path: &str) -> Result<String, Box<dyn Error>> {
    let api_key = std::env::var("API_KEY").expect("API_KEY must be set");
    let upload_url = std::env::var("UPLOAD_URL").expect("UPLOAD_URL must be set");
    let transcript_url = std::env::var("TRANSCRIPT_URL").expect("TRANSCRIPT_URL must be set");

    let client = reqwest::Client::new();

    let mut file = File::open(file_path)?;
    let mut audio_data = Vec::new();
    file.read_to_end(&mut audio_data)?;
    println!("Audio-Datei geladen, Größe: {} Bytes", audio_data.len());

    let upload_response = client
        .post(&upload_url)
        .header("authorization", &api_key)
        .header("content-type", "audio/wav")
        .body(audio_data)
        .send()
        .await?;

    if !upload_response.status().is_success() {
        let error_body = upload_response.text().await?;
        println!("Fehler beim Hochladen: {}", error_body);
        return Err("Fehler beim Hochladen der Audiodatei".into());
    }

    let upload_json = upload_response.json::<Value>().await?;
    println!("Upload erfolgreich: {:?}", upload_json);

    let audio_url = upload_json["upload_url"]
        .as_str()
        .ok_or("Failed to get upload URL")?;
    println!("Audio-URL erhalten: {}", audio_url);

    let transcript_request = client
        .post(&transcript_url)
        .header("authorization", &api_key)
        .json(&serde_json::json!({ "audio_url": audio_url }))
        .send()
        .await?;

    if !transcript_request.status().is_success() {
        let error_body = transcript_request.text().await?;
        println!("Fehler beim Senden des Transkriptionsauftrags: {}", error_body);
        return Err("Fehler beim Anfordern der Transkription".into());
    }

    let transcript_json = transcript_request.json::<Value>().await?;
    let transcript_id = transcript_json["id"]
        .as_str()
        .ok_or("Failed to get transcript ID")?;
    println!("Transkriptionsauftrag erfolgreich, ID: {}", transcript_id);

    loop {
        let status_response = client
            .get(format!("{}/{}", transcript_url, transcript_id))
            .header("authorization", &api_key)
            .send()
            .await?;

        let status_json = status_response.json::<Value>().await?;
        let status = status_json["status"].as_str().unwrap_or("");
        println!("Transkriptionsstatus: {}", status);

        if status == "completed" {
            let transcript_text = status_json["text"].as_str().unwrap_or("");
            println!("Transkription abgeschlossen: {}", transcript_text);
            handle_transcript(transcript_text);
            return Ok(transcript_text.to_string());
        } else if status == "failed" {
            let error_message = status_json["error"].as_str().unwrap_or("Unknown error");
            println!("Transkription fehlgeschlagen: {}", error_message);
            return Err(format!("Transcription failed: {}", error_message).into());
        } else {
            println!("Warte auf Transkriptionsabschluss...");
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        }
    }
}

fn handle_transcript(transcript_text: &str) {
    println!("Handling transcript: {}", transcript_text);

    if transcript_text.to_lowercase().contains("weather") {
        println!("Opening weather app...");
        let status = Command::new("open")
            .arg("/System/Applications/Weather.app")
            .status()
            .expect("Failed to open weather app");

        if !status.success() {
            eprintln!("Error opening weather app: {:?}", status);
        }
    } else if transcript_text.to_lowercase().contains("calculator") {
        println!("Opening calculator...");
        let status = Command::new("open")
            .arg("/System/Applications/Calculator.app")
            .status()
            .expect("Failed to open calculator");

        if !status.success() {
            eprintln!("Error opening calculator: {:?}", status);
        }
    } else {
        println!("No matching action found for transcript.");
    }
}
