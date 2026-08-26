// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// run-foundation-model: run one prompt against the on-device Apple
// Foundation Models. The app spawns this binary; it never runs on its own.
//
// The runner is generic on purpose: the caller supplies the instructions
// and the prompt, so any feature can use it. Feature-specific prompt
// construction lives in the app, not here.
//
// Modes:
// - `run-foundation-model --probe` prints "available" or
//   "unavailable: <reason>" and exits 0.
// - `run-foundation-model` reads one JSON object from stdin
//   (`{"instructions", "prompt"}`), prints the model response to stdout,
//   and exits 0. Any failure exits 1 with an empty stdout.
//
// The build guards apply at two levels. `canImport` compiles a permanent
// "unavailable" stub on an SDK without FoundationModels. `#available` makes
// the same binary run on macOS 13, where the framework does not exist.

import Foundation

#if canImport(FoundationModels)
import FoundationModels
#endif

struct ModelRequest: Decodable {
    let instructions: String
    let prompt: String
}

func availabilityDescription() -> String {
    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        switch SystemLanguageModel.default.availability {
        case .available:
            return "available"
        case .unavailable(let reason):
            return "unavailable: \(reason)"
        @unknown default:
            return "unavailable: unknown"
        }
    }
    return "unavailable: macOS 26 or later is required"
    #else
    return "unavailable: built without FoundationModels"
    #endif
}

func respond(to request: ModelRequest) -> String? {
    #if canImport(FoundationModels)
    guard #available(macOS 26.0, *) else { return nil }
    guard case .available = SystemLanguageModel.default.availability else { return nil }
    let session = LanguageModelSession(instructions: request.instructions)
    // The caller owns the deadline: the app kills this process on timeout.
    let semaphore = DispatchSemaphore(value: 0)
    var response: String?
    Task {
        defer { semaphore.signal() }
        do {
            response = try await session.respond(to: request.prompt).content
        } catch {
            response = nil
        }
    }
    semaphore.wait()
    return response
    #else
    return nil
    #endif
}

if CommandLine.arguments.contains("--probe") {
    print(availabilityDescription())
    exit(0)
}

let input = FileHandle.standardInput.readDataToEndOfFile()
guard let request = try? JSONDecoder().decode(ModelRequest.self, from: input) else {
    exit(1)
}
guard let response = respond(to: request), !response.isEmpty else {
    exit(1)
}
print(response)
exit(0)
