"""Check bundled onboarding examples with Linux WebKitGTK and the shared app CSP."""

import argparse
import json
import mimetypes
import sys
from pathlib import Path
from urllib.parse import urlparse

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
gi.require_version("Soup", "3.0")
from gi.repository import Gio, GLib, Gtk, Soup, WebKit2  # noqa: E402


DESKTOP = Path(__file__).resolve().parents[1]
PARENT = (
    b'<!doctype html><iframe sandbox="allow-scripts" '
    b'src="/onboarding-demo/index.html?bars=4&island=collapsed"></iframe>'
)
PROBE = """(() => {
  const report = value => window.webkit.messageHandlers.probe.postMessage(JSON.stringify(value));
  window.addEventListener('error', event => report({kind: 'error', message: event.message}));
  window.addEventListener('unhandledrejection', () => report({kind: 'error', message: 'Unhandled rejection'}));
  window.addEventListener('securitypolicyviolation', event => report({kind: 'error', message: event.violatedDirective}));
  if (window === top) {
    let state;
    window.setProbeState = value => {
      state = value;
      document.querySelector('iframe')?.contentWindow.postMessage({source: 'antiburn-onboarding', state}, '*');
    };
    window.addEventListener('message', event => {
      if (event.source !== document.querySelector('iframe')?.contentWindow) return;
      if (event.data?.source === 'antiburn-onboarding-demo') report({kind: 'demo', type: event.data.type});
    });
    window.addEventListener('load', () => { if (state) window.setProbeState(state); }, true);
  } else {
    window.addEventListener('message', event => {
      if (event.source !== parent || event.data?.source !== 'antiburn-onboarding') return;
      if (location.pathname !== '/onboarding-demo/index.html') return;
      requestAnimationFrame(() => requestAnimationFrame(() => {
        const root = document.getElementById('root');
        let parentBlocked = false;
        try { void parent.document; } catch { parentBlocked = true; }
        report({kind: 'render', sequence: event.data.state.sequence,
          visible: event.data.state.visible, limits: !!root?.querySelector('.limits-scene'),
          children: root?.childElementCount ?? 0, parentBlocked,
          nativeBridge: typeof window.__TAURI_INTERNALS__ !== 'undefined'});
      }));
    });
  }
})();
"""


class Probe:
    """Serve embedded assets through one protocol handler shared by two windows."""

    def __init__(self, assets, policy, first_window):
        self.assets = assets
        self.policy = policy
        self.first_window = first_window
        self.failed = False
        self.ready = False
        self.cases = [
            dict(slide=1, theme="dark", platform="unsupported", visible=True),
            dict(slide=1, theme="dark", platform="unsupported", visible=False),
            dict(slide=1, theme="dark", platform="unsupported", visible=True),
        ]
        self.index = 0
        self.context = WebKit2.WebContext.new_ephemeral()
        self.context.get_security_manager().register_uri_scheme_as_secure("tauri")
        self.context.register_uri_scheme("tauri", self.resource)
        self.manager = WebKit2.UserContentManager.new()
        self.manager.register_script_message_handler("probe")
        self.manager.connect("script-message-received::probe", self.receive)
        self.manager.add_script(
            WebKit2.UserScript.new(
                PROBE,
                WebKit2.UserContentInjectedFrames.ALL_FRAMES,
                WebKit2.UserScriptInjectionTime.START,
                None,
                None,
            )
        )
        self.windows = []

    def resource(self, request):
        """Apply the configured shared policy, without a per-window override."""
        path = urlparse(request.get_uri()).path
        if path == "/onboarding.html":
            body, mime = PARENT, "text/html"
        elif path == "/popover.html":
            body, mime = b"<!doctype html><title>Prewarmed popover</title>", "text/html"
        elif path.startswith("/onboarding-demo/"):
            file = self.assets / Path(path).name
            if not file.is_file():
                self.fail(f"Missing bundled asset: {file.name}")
                request.finish_error(GLib.Error("Missing bundled asset"))
                return
            body = file.read_bytes()
            mime = mimetypes.guess_type(file.name)[0] or "application/octet-stream"
        else:
            self.fail(f"Unexpected resource: {path}")
            request.finish_error(GLib.Error("Unexpected resource"))
            return
        response = WebKit2.URISchemeResponse.new(
            Gio.MemoryInputStream.new_from_bytes(GLib.Bytes.new(body)), len(body)
        )
        response.set_content_type(mime)
        headers = Soup.MessageHeaders.new(Soup.MessageHeadersType.RESPONSE)
        headers.append("Access-Control-Allow-Origin", "tauri://localhost")
        if mime == "text/html":
            headers.append("Content-Security-Policy", self.policy)
        response.set_http_headers(headers)
        request.finish_with_response(response)

    def fail(self, message):
        """Stop the scenario and retain a failing exit status."""
        self.failed = True
        print(f"FAIL [{self.first_window} first]: {message}", flush=True)
        Gtk.main_quit()

    def send(self):
        state = dict(
            self.cases[self.index],
            sequence=self.index,
            step=0,
            elapsed=0,
            transferProgress=0,
            paused=False,
        )
        self.example.evaluate_javascript(
            f"window.setProbeState({json.dumps(state)})", -1, None, None, None, None, None
        )
        return False

    def receive(self, _manager, result):
        """Require readiness, rendered content, and isolation for each scene."""
        if self.failed or self.index >= len(self.cases):
            return
        message = json.loads(result.get_js_value().to_string())
        if message["kind"] == "error":
            self.fail(message["message"])
        elif message["kind"] == "demo":
            if message["type"] == "error":
                self.fail("The demo reported a runtime error")
            elif message["type"] == "ready":
                self.ready = True
        elif message["kind"] == "render":
            if not self.ready:
                return
            if message["sequence"] != self.index:
                return
            case = self.cases[self.index]
            if (
                bool(message["children"]) != case["visible"]
                or message["limits"] != case["visible"]
                or not message["parentBlocked"]
                or message["nativeBridge"]
            ):
                self.fail(f"Unexpected render or isolation state: {message}")
                return
            self.index += 1
            if self.index == len(self.cases):
                if not self.ready:
                    self.fail("The demo did not acknowledge readiness")
                else:
                    print(
                        f"PASS [{self.first_window} first]: Limits loads, hides, and resumes",
                        flush=True,
                    )
                    Gtk.main_quit()
            else:
                GLib.idle_add(self.send)

    def run(self):
        """Exercise both creation orders in a fresh WebKit context."""
        names = ["popover", "onboarding"]
        if self.first_window == "onboarding":
            names.reverse()
        for name in names:
            if name == "onboarding":
                view = WebKit2.WebView(
                    web_context=self.context, user_content_manager=self.manager
                )
                self.example = view
                view.get_settings().set_enable_write_console_messages_to_stdout(True)
                view.connect(
                    "load-changed",
                    lambda _view, event: self.send()
                    if event == WebKit2.LoadEvent.FINISHED
                    else None,
                )
            else:
                view = WebKit2.WebView(web_context=self.context)
            window = Gtk.Window()
            window.set_default_size(860, 620)
            window.add(view)
            window.show_all()
            self.windows.append(window)
            view.load_uri(f"tauri://localhost/{name}.html")

        def expired():
            self.timeout = None
            self.fail("Startup or render timed out")
            return False

        self.timeout = GLib.timeout_add_seconds(20, expired)
        Gtk.main()
        if self.timeout is not None:
            GLib.source_remove(self.timeout)
        for window in self.windows:
            window.destroy()
        return not self.failed and self.index == len(self.cases)


def main():
    """Read the shipping policy and assets, then check both window orders."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, default=DESKTOP / "dist/onboarding-demo")
    parser.add_argument("--config", type=Path, default=DESKTOP / "src-tauri/tauri.conf.json")
    args = parser.parse_args()
    policy = json.loads(args.config.read_text())["app"]["security"]["csp"]
    if not (args.assets / "index.html").is_file():
        parser.error("Build the desktop frontend before running this check")
    print(
        f"WebKitGTK {WebKit2.get_major_version()}."
        f"{WebKit2.get_minor_version()}.{WebKit2.get_micro_version()}",
        flush=True,
    )
    return all(Probe(args.assets, policy, order).run() for order in ("popover", "onboarding"))


if __name__ == "__main__":
    sys.exit(0 if main() else 1)
