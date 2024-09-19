let isRecording = false;
let backgroundVideo;

function onInit() {
    backgroundVideo = document.getElementById('background-video');
    backgroundVideo.pause();
}

function toogleRecording() {
    if (!isRecording) {
        startRecording();
    } else {
        stopRecording();
    }
}

async function startRecording() {
    isRecording = true;
    backgroundVideo.play();

    try {
        const response = await fetch('/record', { method: 'POST' });
        const result = await response.text();
        console.log(result); // Hier kannst du das Ergebnis verarbeiten
    } catch (error) {
        console.error('Error:', error);
    }
}

function stopRecording() {
    isRecording = false;
    backgroundVideo.pause();
    fetch('/stop_recording', { method: 'POST' })
        .then(response => response.text())
        .then(result => console.log(result))
        .catch(error => console.error('Error:', error));
}
