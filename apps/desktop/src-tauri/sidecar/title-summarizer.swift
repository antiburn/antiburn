// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// title-summarizer: name one coding-agent session with the on-device Apple
// Foundation Models. The app spawns this binary; it never runs on its own.
//
// Modes:
// - `title-summarizer --probe` prints "available" or "unavailable: <reason>"
//   and exits 0.
// - `title-summarizer` reads one JSON object from stdin
//   (`{"repo", "firstMessage", "context"}`), prints the generated title to
//   stdout, and exits 0. Any failure exits 1 with an empty stdout, and the
//   app keeps the fallback title.
//
// The build guards apply at two levels. `canImport` compiles a permanent
// "unavailable" stub on an SDK without FoundationModels. `#available` makes
// the same binary run on macOS 13, where the framework does not exist.

import Foundation

#if canImport(FoundationModels)
import FoundationModels
#endif

struct TitleRequest: Decodable {
    let repo: String?
    let firstMessage: String
    let context: [String]
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

func prompt(for request: TitleRequest) -> String {
    var lines: [String] = []
    if let repo = request.repo, !repo.isEmpty {
        lines.append("Repository: \(repo)")
    }
    lines.append("First message: \(request.firstMessage)")
    if !request.context.isEmpty {
        lines.append("Later messages:")
        for message in request.context {
            lines.append("- \(message)")
        }
    }
    return lines.joined(separator: "\n")
}

func generateTitle(for request: TitleRequest) -> String? {
    #if canImport(FoundationModels)
    guard #available(macOS 26.0, *) else { return nil }
    guard case .available = SystemLanguageModel.default.availability else { return nil }
    let session = LanguageModelSession(
        instructions: """
        You name coding-agent chat sessions. The user shows you the opening \
        messages of one session. Name the session for its main task: the \
        first message states it, and later messages only add context. Reply \
        with only the title: 3 to 6 words, imperative mood, no quotes, no \
        trailing period.
        """
    )
    // The caller owns the deadline: the app kills this process on timeout.
    let semaphore = DispatchSemaphore(value: 0)
    var title: String?
    Task {
        defer { semaphore.signal() }
        do {
            let response = try await session.respond(to: prompt(for: request))
            title = response.content
        } catch {
            title = nil
        }
    }
    semaphore.wait()
    return title
    #else
    return nil
    #endif
}

if CommandLine.arguments.contains("--probe") {
    print(availabilityDescription())
    exit(0)
}

let input = FileHandle.standardInput.readDataToEndOfFile()
guard let request = try? JSONDecoder().decode(TitleRequest.self, from: input) else {
    exit(1)
}
guard let title = generateTitle(for: request), !title.isEmpty else {
    exit(1)
}
print(title)
exit(0)
