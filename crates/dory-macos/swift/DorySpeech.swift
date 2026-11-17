// Apple Speech for Dory: SpeechTranscriber locales, their assets, and transcription of one range
// of an audio file. Follows DoryAI.swift's rules: every call blocks, never traps, and runs its
// callbacks on the calling thread.

import AVFAudio
import CoreMedia
import Foundation
import Speech

/// One locale's assets as `dory_speech_asset_state` reports them. `src/ai/ffi.rs` mirrors it.
enum DorySpeechAsset: Int32 {
  case installed = 0
  case supported = 1
  case downloading = 2
  case unsupported = 3
}

/// A failure this file names itself, with the code the caller sees.
struct DorySpeechFailure: Error {
  let code: DoryCode
  let message: String
}

/// Runs `body` on Swift's pool and blocks this thread until it returns. Never call it from a
/// Swift concurrency thread.
func blockOn<T>(_ fallback: T, _ body: @escaping @Sendable () async -> T) -> T {
  let pump = Pump<T>()
  Task.detached {
    pump.send(await body())
    pump.finish()
  }
  var value = fallback
  _ = pump.drain({ value = $0; return true }, onStop: {})
  return value
}

func makeTranscriber(_ locale: Locale) -> SpeechTranscriber {
  SpeechTranscriber(
    locale: locale, transcriptionOptions: [], reportingOptions: [],
    attributeOptions: [.audioTimeRange])
}

func classifySpeech(_ error: any Error) -> (DoryCode, String) {
  if error is CancellationError { return (.cancelled, "cancelled") }
  if let failure = error as? DorySpeechFailure { return (failure.code, failure.message) }
  let message = error.localizedDescription
  if let speech = error as? SFSpeechError {
    let code = speech.code
    if code == .noModel || code == .assetLocaleNotAllocated { return (.speechAssetsMissing, message) }
    if code == .cannotAllocateUnsupportedLocale { return (.speechUnsupportedLocale, message) }
    if code == .tooManyAssetLocalesAllocated {
      return (.speechReservationLimit, String(AssetInventory.maximumReservedLocales))
    }
    if code == .audioDisordered || code == .unexpectedAudioFormat
      || code == .incompatibleAudioFormats
    {
      return (.speechAudioUnreadable, message)
    }
    if code == .insufficientResources { return (.speechBusy, message) }
  }
  if (error as NSError).domain == NSURLErrorDomain { return (.speechNetwork, message) }
  return (.other, message)
}

// MARK: Locales and assets

@_cdecl("dory_speech_locales")
public func dorySpeechLocales(_ csv: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 {
  let tags = blockOn([String]()) {
    guard SpeechTranscriber.isAvailable else { return [] }
    return await SpeechTranscriber.supportedLocales.map { $0.identifier(.bcp47) }
  }
  put(tags.sorted().joined(separator: ","), into: csv)
  return DoryCode.ok.rawValue
}

@_cdecl("dory_speech_asset_state")
public func dorySpeechAssetState(_ locale: UnsafePointer<CChar>?) -> Int32 {
  guard let locale else { return DorySpeechAsset.unsupported.rawValue }
  let id = String(cString: locale)
  return blockOn(DorySpeechAsset.unsupported) { await assetState(Locale(identifier: id)) }.rawValue
}

/// `AssetInventory.status` says `supported` for a locale macOS installed for itself until this app
/// reserves it, yet such a locale transcribes, so `installedLocales` decides what is installed.
func assetState(_ locale: Locale) async -> DorySpeechAsset {
  let id = locale.identifier(.bcp47)
  if await SpeechTranscriber.installedLocales.contains(where: { $0.identifier(.bcp47) == id }) {
    return .installed
  }
  switch await AssetInventory.status(forModules: [makeTranscriber(locale)]) {
  case .installed: return .installed
  case .downloading: return .downloading
  case .supported: return .supported
  case .unsupported: return .unsupported
  @unknown default: return .unsupported
  }
}

// MARK: Install

@_cdecl("dory_speech_install")
public func dorySpeechInstall(
  _ handle: UnsafeMutableRawPointer?,
  _ locale: UnsafePointer<CChar>?,
  _ ctx: UnsafeMutableRawPointer?,
  _ onProgress: (@convention(c) (UnsafeMutableRawPointer?, Double) -> Bool)?,
  _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
  error?.pointee = nil
  guard let handle, let locale else {
    put("a task and a locale are required", into: error)
    return DoryCode.invalidArgument.rawValue
  }
  let task = Unmanaged<DoryTask>.fromOpaque(handle).takeUnretainedValue()
  let id = String(cString: locale)
  let pump = Pump<Double>()
  let outcome = Outcome()

  task.start {
    do {
      try await install(Locale(identifier: id), pump: pump)
      try Task.checkCancellation()
      outcome.code = .ok
    } catch {
      (outcome.code, outcome.message) =
        Task.isCancelled ? (.cancelled, "cancelled") : classifySpeech(error)
    }
    pump.finish()
  }

  let kept = pump.drain(
    { fraction in onProgress?(ctx, fraction) ?? true },
    onStop: { task.cancel() })
  return finish(outcome, kept: kept, error: error)
}

func install(_ locale: Locale, pump: Pump<Double>) async throws {
  let transcriber = makeTranscriber(locale)
  switch await assetState(locale) {
  case .installed:
    pump.send(1)
    return
  case .unsupported:
    throw DorySpeechFailure(
      code: .speechUnsupportedLocale, message: "\(locale.identifier(.bcp47)) is not supported")
  default:
    break
  }
  // Checked here because whether Apple's request throws at the limit is not documented.
  let reserved = await AssetInventory.reservedLocales
  let limit = AssetInventory.maximumReservedLocales
  let id = locale.identifier(.bcp47)
  if reserved.count >= limit && !reserved.contains(where: { $0.identifier(.bcp47) == id }) {
    throw DorySpeechFailure(code: .speechReservationLimit, message: String(limit))
  }
  guard let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber])
  else {
    pump.send(1)
    return
  }
  let observation = request.progress.observe(\.fractionCompleted, options: [.initial, .new]) {
    progress, _ in pump.send(progress.fractionCompleted)
  }
  defer { observation.invalidate() }
  try await withTaskCancellationHandler {
    try await request.downloadAndInstall()
  } onCancel: {
    request.progress.cancel()
  }
}

// MARK: Transcribe

enum SpeechEvent {
  case segment(startMs: UInt64, endMs: UInt64, text: String)
  case progress(Double)
}

/// Times from `on_segment` are relative to `start_ms`. `end_ms` past the file's end reads to it.
@_cdecl("dory_speech_transcribe")
public func dorySpeechTranscribe(
  _ handle: UnsafeMutableRawPointer?,
  _ path: UnsafePointer<CChar>?,
  _ startMs: UInt64,
  _ endMs: UInt64,
  _ locale: UnsafePointer<CChar>?,
  _ ctx: UnsafeMutableRawPointer?,
  _ onSegment: (@convention(c) (UnsafeMutableRawPointer?, UInt64, UInt64, UnsafePointer<CChar>?) ->
    Bool)?,
  _ onProgress: (@convention(c) (UnsafeMutableRawPointer?, Double) -> Bool)?,
  _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
  error?.pointee = nil
  guard let handle, let path, let locale, endMs >= startMs else {
    put("a task, a path, a locale, and a range that does not end before it starts are required",
      into: error)
    return DoryCode.invalidArgument.rawValue
  }
  let task = Unmanaged<DoryTask>.fromOpaque(handle).takeUnretainedValue()
  let url = URL(fileURLWithPath: String(cString: path))
  let id = String(cString: locale)
  let pump = Pump<SpeechEvent>()
  let outcome = Outcome()

  task.start {
    do {
      try await transcribe(
        url, from: startMs, to: endMs, locale: Locale(identifier: id), task: task, pump: pump)
      try Task.checkCancellation()
      outcome.code = .ok
    } catch {
      (outcome.code, outcome.message) =
        Task.isCancelled || task.isCancelled ? (.cancelled, "cancelled") : classifySpeech(error)
    }
    pump.finish()
  }

  let kept = pump.drain(
    { event in
      switch event {
      case .segment(let start, let end, let text):
        guard let onSegment else { return true }
        return text.withCString { onSegment(ctx, start, end, $0) }
      case .progress(let fraction):
        return onProgress?(ctx, fraction) ?? true
      }
    },
    onStop: { task.cancel() })
  return finish(outcome, kept: kept, error: error)
}

func finish(
  _ outcome: Outcome, kept: Bool, error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
  if !kept {
    outcome.code = .cancelled
    outcome.message = "cancelled"
  }
  if outcome.code != .ok { put(outcome.message, into: error) }
  return outcome.code.rawValue
}

func milliseconds(_ seconds: Double) -> UInt64 {
  guard seconds.isFinite, seconds > 0 else { return 0 }
  return UInt64(min(seconds * 1000, 1e15).rounded())
}

func transcribe(
  _ url: URL, from startMs: UInt64, to endMs: UInt64, locale: Locale, task: DoryTask,
  pump: Pump<SpeechEvent>
) async throws {
  let file: AVAudioFile
  do {
    file = try AVAudioFile(forReading: url)
  } catch {
    throw DorySpeechFailure(
      code: .speechAudioUnreadable,
      message: "cannot read \(url.path): \(error.localizedDescription)")
  }
  let transcriber = makeTranscriber(locale)
  guard
    let format = await SpeechAnalyzer.bestAvailableAudioFormat(
      compatibleWith: [transcriber], considering: file.processingFormat)
  else {
    throw DorySpeechFailure(
      code: .speechUnsupportedLocale, message: "no audio format for \(locale.identifier(.bcp47))")
  }
  let reader = try RangeReader(file: file, startMs: startMs, endMs: endMs, to: format, task: task)
  let seconds = reader.seconds
  let analyzer = SpeechAnalyzer(modules: [transcriber])

  try await withTaskCancellationHandler {
    async let collected: Void = {
      for try await result in transcriber.results {
        let start = result.range.start.seconds
        let end = result.range.end.seconds
        pump.send(
          .segment(
            startMs: milliseconds(start), endMs: milliseconds(end),
            text: String(result.text.characters)))
        if seconds > 0, end.isFinite { pump.send(.progress(min(1, max(0, end / seconds)))) }
      }
    }()
    do {
      if let last = try await analyzer.analyzeSequence(AsyncStream(unfolding: reader.next)) {
        try await analyzer.finalizeAndFinish(through: last)
      } else {
        await analyzer.cancelAndFinishNow()
      }
    } catch {
      await analyzer.cancelAndFinishNow()
      throw error
    }
    try await collected
  } onCancel: {
    Task { await analyzer.cancelAndFinishNow() }
  }
  if let failure = reader.failure { throw failure }
}

/// Reads one range of a file in the analyzer's format, a second at a time, when the analyzer
/// asks. Pulling keeps at most one chunk in memory however long the range is.
final class RangeReader: @unchecked Sendable {
  private let file: AVAudioFile
  private let format: AVAudioFormat
  private let converter: AVAudioConverter?
  private let task: DoryTask
  private let chunk: AVAudioFrameCount
  private var remaining: AVAudioFramePosition
  private(set) var failure: (any Error)?
  let seconds: Double

  init(file: AVAudioFile, startMs: UInt64, endMs: UInt64, to format: AVAudioFormat, task: DoryTask)
    throws
  {
    let source = file.processingFormat
    let length = file.length
    guard source.sampleRate.isFinite, source.sampleRate > 0, format.sampleRate.isFinite,
      format.sampleRate > 0
    else {
      throw DorySpeechFailure(code: .speechAudioUnreadable, message: "the audio has no sample rate")
    }
    let frame = { (ms: UInt64) -> AVAudioFramePosition in
      AVAudioFramePosition(min(Double(ms) * source.sampleRate / 1000, Double(length)))
    }
    let start = frame(startMs)
    let end = max(start, frame(endMs))
    file.framePosition = start
    self.file = file
    self.format = format
    self.task = task
    self.remaining = end - start
    self.chunk = AVAudioFrameCount(min(max(1, source.sampleRate.rounded()), 1e7))
    self.seconds = source.sampleRate > 0 ? Double(end - start) / source.sampleRate : 0
    if source == format {
      converter = nil
    } else if let converter = AVAudioConverter(from: source, to: format) {
      self.converter = converter
    } else {
      throw DorySpeechFailure(
        code: .speechAudioUnreadable, message: "cannot convert \(source) to \(format)")
    }
  }

  @Sendable func next() async -> AnalyzerInput? {
    guard failure == nil, !task.isCancelled, remaining > 0 else { return nil }
    let count = AVAudioFrameCount(min(remaining, AVAudioFramePosition(chunk)))
    do {
      guard let input = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: count)
      else {
        throw DorySpeechFailure(code: .other, message: "cannot allocate an audio buffer")
      }
      try file.read(into: input, frameCount: count)
      guard input.frameLength > 0 else { return nil }
      remaining -= AVAudioFramePosition(input.frameLength)
      return AnalyzerInput(buffer: try convert(input))
    } catch {
      failure = error
      return nil
    }
  }

  private func convert(_ input: AVAudioPCMBuffer) throws -> AVAudioPCMBuffer {
    guard let converter else { return input }
    let ratio = format.sampleRate / input.format.sampleRate
    let capacity = AVAudioFrameCount(min((Double(input.frameLength) * ratio).rounded(.up), 1e8)) + 1024
    guard let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else {
      throw DorySpeechFailure(code: .other, message: "cannot allocate an audio buffer")
    }
    var fed = false
    var problem: NSError?
    let status = converter.convert(to: output, error: &problem) { _, inputStatus in
      if fed {
        inputStatus.pointee = .noDataNow
        return nil
      }
      fed = true
      inputStatus.pointee = .haveData
      return input
    }
    if status == .error {
      throw DorySpeechFailure(
        code: .speechAudioUnreadable,
        message: problem?.localizedDescription ?? "audio conversion failed")
    }
    return output
  }
}
