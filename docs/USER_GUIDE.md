# Dory user guide

Dory records a meeting from your Mac, turns it into a speaker-labelled transcript, and writes a short summary. Everything runs on your Mac. This guide covers the native app on macOS 26 or later.

## First run

Onboarding has three short steps.

1. **Welcome.** Pick the languages your meetings use. Dory only picks a speech model that covers all of them.
2. **Permissions.** Grant **Microphone** and **System Audio Recording**. Dory explains each one before macOS asks. If you return from System Settings, the status updates by itself.
3. **Models.** Dory downloads the default speech model and shows its size. Summaries are optional and need [Ollama](https://ollama.com). Choose **Set up summaries** to see the commands, or skip it.

A missing summary model never blocks recording. A missing speech model does not either, and those recordings wait in the queue until the model is ready.

## Record a meeting

Press **Start Recording** or use File > New Recording (Command-Shift-R). Dory captures your microphone and the system audio together. It never changes your default input or output device, so your volume keys keep working.

While recording, the toolbar shows an elapsed-time pill with a level meter, **Pause**, and **Stop**. The menu-bar item shows a red dot and the elapsed time. A recording stops at two hours. Five minutes before that, the pill turns amber and Dory posts a notification.

**Stop** saves at once and offers **Undo** for 5 seconds. Undo resumes the same recording. Processing starts after the 5 seconds pass.

## Read the result

The sidebar lists recordings by day. Each row shows the title, start time, duration, and a status. The search field filters by title, transcript, and summary text.

Select a recording to see its detail pane.

- **Summary** has key points, decisions, and action items. **Copy** and **Export** sit above it.
- **Transcript** groups lines by speaker turn with timestamps. Press Return on a turn, or click its timestamp, to play from there. Rename a speaker once and the name applies to the whole recording.

Use Command-1 and Command-2 to switch tabs. File > Export writes Markdown, plain text, or SRT subtitles.

## Processing

By default a stopped recording is transcribed and summarized automatically. Turn that off in **Settings > Transcription** and Dory asks what to do after each recording instead.

The row and the detail pane show the stage ("Transcribing, 40%") and an estimate of the time left. **Settings > Transcription > Processing** sets the pace.

| Pace | Effect |
| --- | --- |
| Automatic | Fast when plugged in and cool. Light on battery, in Low Power Mode, when the Mac is hot, and while you record. |
| Fast | Up to 4 cores. Finishes soonest, and the fans may spin up. |
| Light | 2 cores. Takes longer and is easier on the battery. |

Only one recording is processed at a time. **Window > Processing Queue** lists the jobs and lets you move one to the top or cancel it. When a job finishes and the window is not in front, Dory posts a notification. Clicking it opens the recording.

## Settings

Open Settings with Command-Comma.

- **General.** Appearance follows the system.
- **Recording.** Choose the microphone.
- **Transcription.** Pick the speech model, the meeting languages, the language mode, the pace, and whether to process automatically. Each model has an info button that says what it is good at and what to watch for. **Add model file** lets you use your own model.
- **Summaries.** Pick the summary model.
- **Storage.** Shows the data folder and its size, with Show in Finder.
- **Assistants.** Lets an assistant such as Claude Code, Claude Desktop, Cursor, or Codex read your meetings. Each assistant gets its own key and reads only the level you choose: titles, summaries, or full transcripts. The pane also lists the connected assistants and what they read last.
- **Advanced.** Holds the speech vocabulary, described below.

### Speech vocabulary

Dory sometimes mishears a name or a product, such as "Lena" for "Lina". Add the word under **Settings > Advanced > Speech vocabulary** and Dory listens harder for it when it transcribes. Type the word, then press Return or choose **Add**. A word cannot contain a slash or a colon. **Remove** takes it out again.

Each word is **Normal** or **Strong**. Normal favors the word gently, and it fixes most names. Switch a word to **Strong** when Normal does not fix it. Strong pushes much harder, but it can turn words that sound alike into your word, so use it only for a word that needs it.

When you rename a speaker in any recording, Dory adds that name to the vocabulary too, up to the 100 newest. The names appear under the switch **Add names you give speakers**. Choose **Hide** next to a name Dory should not use, such as a name you typed by mistake, and **Show** brings it back. Turn the switch off to stop adding names.

Changes apply to recordings transcribed from now on. To use them on an older recording, re-transcribe it. Transcribing takes a little longer while the list has words in it. The vocabulary only changes the words in the transcript, and the summary is written from those words.

Only some speech models use the vocabulary. When yours does not, the pane says so and names a model that does. You can pick another model in **Settings > Transcription**.

## Keyboard shortcuts

| Command | Shortcut |
| --- | --- |
| New recording | Command-Shift-R |
| Pause or resume | Command-Shift-P |
| Stop | Command-Period |
| Play or pause | Space |
| Skip 5 seconds | Left Arrow, Right Arrow in the player |
| Find | Command-F |
| Summary or Transcript | Command-1, Command-2 |
| Copy summary | Command-Shift-C |
| Export | Command-E |
| Rename | Return on a selected row |
| Move to Trash | Command-Delete |
| Settings | Command-Comma |
| Next or previous area | F6, Shift-F6 |

## Your data

Dory keeps everything in `~/Library/Application Support/com.dory.app/v2`. Deleting a recording moves it to the Trash, and Command-Z brings it back. Nothing is uploaded.

## Troubleshooting

**A permission shows Not allowed.** Open System Settings > Privacy & Security, switch on Dory under Microphone and System Audio Recording, then return to Dory. A build signed ad hoc loses these grants each time it is rebuilt.

**The system channel is silent.** Check that System Audio Recording is allowed for Dory, and that something is playing. Dory records the audio other apps send to your speakers.

**Summaries are missing.** Open **Set up summaries** on the recording. Ollama must be running and the Gemma model must be installed.

**A recording is stuck.** Open Window > Processing Queue, cancel the job, and start it again from the recording's detail pane.

**Upgrading from the old Dory.** Your recordings are imported on first launch. See the README for what changes.
