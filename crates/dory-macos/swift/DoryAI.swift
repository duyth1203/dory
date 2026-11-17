// Dory's bridge to Apple's Swift-only frameworks, compiled by build.rs into libdoryai.a.
//
// Every exported function blocks its calling thread and never traps: no force-unwraps, every
// thrown error is caught, and a bad argument is reported as a code. Strings this file returns are
// malloc'd UTF-8 that the caller hands back to `dory_string_free`. A callback runs on the calling
// thread, between waits, so the caller's context is never touched from Swift's thread pool.

import Foundation
import FoundationModels

/// Result codes shared by every call. `src/ai/ffi.rs` mirrors them.
enum DoryCode: Int32 {
  case ok = 0
  case cancelled = 1
  case invalidArgument = 2
  case exceededContextWindowSize = 10
  case assetsUnavailable = 11
  case guardrailViolation = 12
  case unsupportedGuide = 13
  case unsupportedLanguageOrLocale = 14
  case decodingFailure = 15
  case rateLimited = 16
  case concurrentRequests = 17
  case refusal = 18
  case otherGenerationError = 19
  case other = 20
}

/// Apple's availability as `dory_fm_info` reports it. `src/ai/ffi.rs` mirrors it.
enum DoryFmState: Int32 {
  case available = 0
  case appleIntelligenceNotEnabled = 1
  case deviceNotEligible = 2
  case modelNotReady = 3
  case osTooOld = 4
  case unknown = 9
}

// MARK: Tasks

/// A cancel handle shared by the blocked caller and any thread that cancels it.
final class DoryTask: @unchecked Sendable {
  private let lock = NSLock()
  private var cancelled = false
  private var running: Task<Void, Never>?

  var isCancelled: Bool {
    lock.lock()
    defer { lock.unlock() }
    return cancelled
  }

  func cancel() {
    lock.lock()
    cancelled = true
    let task = running
    lock.unlock()
    task?.cancel()
  }

  /// Runs `body` in a new Swift task tied to this handle, cancelled at once if the handle already was.
  func start(_ body: @escaping @Sendable () async -> Void) {
    let task = Task.detached(operation: body)
    lock.lock()
    running = task
    let cancelNow = cancelled
    lock.unlock()
    if cancelNow { task.cancel() }
  }
}

@_cdecl("dory_task_new")
public func doryTaskNew() -> UnsafeMutableRawPointer {
  Unmanaged.passRetained(DoryTask()).toOpaque()
}

@_cdecl("dory_task_cancel")
public func doryTaskCancel(_ handle: UnsafeMutableRawPointer?) {
  guard let handle else { return }
  Unmanaged<DoryTask>.fromOpaque(handle).takeUnretainedValue().cancel()
}

@_cdecl("dory_task_free")
public func doryTaskFree(_ handle: UnsafeMutableRawPointer?) {
  guard let handle else { return }
  Unmanaged<DoryTask>.fromOpaque(handle).release()
}

@_cdecl("dory_string_free")
public func doryStringFree(_ string: UnsafeMutablePointer<CChar>?) {
  free(string)
}

// MARK: Pump

/// Carries events from a Swift task to the blocked calling thread, which hands them to the
/// caller's callback.
final class Pump<Event>: @unchecked Sendable {
  private let lock = NSLock()
  private let ready = DispatchSemaphore(value: 0)
  private var pending: [Event] = []
  private var finished = false

  func send(_ event: Event) {
    lock.lock()
    pending.append(event)
    lock.unlock()
    ready.signal()
  }

  func finish() {
    lock.lock()
    finished = true
    lock.unlock()
    ready.signal()
  }

  /// Blocks until `finish`. When `deliver` returns false, calls `onStop` once, drops later events,
  /// and still waits for the task to finish. Returns false when it stopped.
  func drain(_ deliver: (Event) -> Bool, onStop: () -> Void) -> Bool {
    var keepGoing = true
    while true {
      ready.wait()
      lock.lock()
      let batch = pending
      pending.removeAll()
      let done = finished
      lock.unlock()
      if keepGoing {
        for event in batch where !deliver(event) {
          keepGoing = false
          onStop()
          break
        }
      }
      if done { return keepGoing }
    }
  }
}

/// What a task leaves for the blocked caller. Written before `Pump.finish`, read after `drain`.
final class Outcome: @unchecked Sendable {
  var code = DoryCode.other
  var text = ""
  var message = ""
}

func put(_ string: String, into out: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) {
  out?.pointee = strdup(string)
}

// MARK: Foundation Models

@_cdecl("dory_fm_info")
public func doryFmInfo(
  _ state: UnsafeMutablePointer<Int32>?,
  _ contextSize: UnsafeMutablePointer<UInt32>?,
  _ languagesCsv: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
  let model = SystemLanguageModel.default
  let reported: DoryFmState
  switch model.availability {
  case .available:
    if #available(macOS 26.4, *) { reported = .available } else { reported = .osTooOld }
  case .unavailable(.appleIntelligenceNotEnabled): reported = .appleIntelligenceNotEnabled
  case .unavailable(.deviceNotEligible): reported = .deviceNotEligible
  case .unavailable(.modelNotReady): reported = .modelNotReady
  case .unavailable: reported = .unknown
  }
  state?.pointee = reported.rawValue
  contextSize?.pointee = UInt32(clamping: model.contextSize)
  let languages = model.supportedLanguages.map { $0.minimalIdentifier }.sorted()
  put(languages.joined(separator: ","), into: languagesCsv)
  return DoryCode.ok.rawValue
}

func classify(_ error: any Error) -> (DoryCode, String) {
  if error is CancellationError { return (.cancelled, "cancelled") }
  guard let error = error as? LanguageModelSession.GenerationError else {
    return (.other, String(describing: error))
  }
  let code: DoryCode
  let context: LanguageModelSession.GenerationError.Context
  switch error {
  case .exceededContextWindowSize(let c): (code, context) = (.exceededContextWindowSize, c)
  case .assetsUnavailable(let c): (code, context) = (.assetsUnavailable, c)
  case .guardrailViolation(let c): (code, context) = (.guardrailViolation, c)
  case .unsupportedGuide(let c): (code, context) = (.unsupportedGuide, c)
  case .unsupportedLanguageOrLocale(let c): (code, context) = (.unsupportedLanguageOrLocale, c)
  case .decodingFailure(let c): (code, context) = (.decodingFailure, c)
  case .rateLimited(let c): (code, context) = (.rateLimited, c)
  case .concurrentRequests(let c): (code, context) = (.concurrentRequests, c)
  case .refusal(_, let c): (code, context) = (.refusal, c)
  @unknown default: return (.otherGenerationError, String(describing: error))
  }
  return (code, "\(error.localizedDescription) \(context.debugDescription)")
}

@_cdecl("dory_fm_complete")
public func doryFmComplete(
  _ handle: UnsafeMutableRawPointer?,
  _ system: UnsafePointer<CChar>?,
  _ user: UnsafePointer<CChar>?,
  _ maxOut: UInt32,
  _ temperature: Float,
  _ ctx: UnsafeMutableRawPointer?,
  _ onPiece: (@convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?, Int) -> Bool)?,
  _ full: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?,
  _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32 {
  full?.pointee = nil
  error?.pointee = nil
  guard let handle, let system, let user else {
    put("a task, system text, and user text are required", into: error)
    return DoryCode.invalidArgument.rawValue
  }
  let task = Unmanaged<DoryTask>.fromOpaque(handle).takeUnretainedValue()
  let instructions = String(cString: system)
  let prompt = String(cString: user)
  let options = GenerationOptions(
    temperature: Double(temperature), maximumResponseTokens: Int(maxOut))
  let pump = Pump<String>()
  let outcome = Outcome()

  task.start {
    var text = ""
    do {
      let model = SystemLanguageModel(guardrails: .permissiveContentTransformations)
      let session = LanguageModelSession(model: model, instructions: instructions)
      for try await snapshot in session.streamResponse(to: prompt, options: options) {
        let now = snapshot.content
        if now.hasPrefix(text) {
          let piece = String(now.dropFirst(text.count))
          if !piece.isEmpty { pump.send(piece) }
        }
        text = now
      }
      try Task.checkCancellation()
      outcome.code = .ok
    } catch {
      (outcome.code, outcome.message) =
        Task.isCancelled ? (.cancelled, "cancelled") : classify(error)
    }
    outcome.text = text
    pump.finish()
  }

  let kept = pump.drain(
    { piece in
      guard let onPiece else { return true }
      return piece.withCString { onPiece(ctx, $0, piece.utf8.count) }
    },
    onStop: { task.cancel() })

  if !kept {
    outcome.code = .cancelled
    outcome.message = "cancelled"
  }
  put(outcome.text, into: full)
  if outcome.code != .ok { put(outcome.message, into: error) }
  return outcome.code.rawValue
}
