import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

export const AISLOP_BIN = fileURLToPath(
  new URL("../node_modules/.bin/aislop", import.meta.url),
);

function plural(count, noun) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

function oneLine(value) {
  const text = String(value).replace(/\s+/g, " ").trim();
  return text.length <= 240 ? text : `${text.slice(0, 237)}...`;
}

function findingLine(finding) {
  const severity = finding.severity === "error" ? "error" : "warning";
  const line = Number.isInteger(finding.line) ? `:${finding.line}` : "";
  const col = Number.isInteger(finding.col) ? `:${finding.col}` : "";
  return `${severity} ${oneLine(finding.file)}${line}${col} ${oneLine(finding.ruleId)}: ${oneLine(finding.message)}`;
}

function readCounts(feedback, findings) {
  const counts = feedback.counts;
  const values = [counts?.error, counts?.warning, counts?.total];
  if (!values.every((value) => Number.isInteger(value) && value >= 0)) {
    throw new Error("invalid aislop counts");
  }
  if (counts.error + counts.warning !== counts.total || findings.length > counts.total) {
    throw new Error("inconsistent aislop counts");
  }

  const shownErrors = findings.filter(({ severity }) => severity === "error").length;
  const shownWarnings = findings.length - shownErrors;
  if (shownErrors > counts.error || shownWarnings > counts.warning) {
    throw new Error("inconsistent aislop findings");
  }
  return counts;
}

export function compactAislopOutput(stdout) {
  if (stdout.trim() === "") return "";

  const envelope = JSON.parse(stdout);
  const additionalContext = envelope?.hookSpecificOutput?.additionalContext;
  if (typeof additionalContext !== "string") {
    throw new Error("missing additionalContext");
  }

  const feedback = JSON.parse(additionalContext);
  if (feedback?.schema !== "aislop.hook.v2" || !Array.isArray(feedback.findings)) {
    throw new Error("invalid aislop feedback");
  }

  const findings = feedback.findings;
  const valid = findings.every(
    (finding) =>
      finding &&
      (finding.severity === "error" || finding.severity === "warning") &&
      typeof finding.file === "string" &&
      typeof finding.ruleId === "string" &&
      typeof finding.message === "string",
  );
  if (!valid) throw new Error("invalid aislop finding");

  const { error: errors, warning: warnings, total } = readCounts(feedback, findings);
  if (total === 0) return "";
  const counts = [];
  if (errors > 0) counts.push(plural(errors, "error"));
  if (warnings > 0) counts.push(plural(warnings, "warning"));
  if (counts.length === 0) counts.push(plural(total, "finding"));

  const shown = findings.length < total ? ` (${findings.length} shown)` : "";
  const context = [`aislop: ${counts.join(", ")}${shown}`, ...findings.map(findingLine)].join(
    "\n",
  );

  return JSON.stringify({
    hookSpecificOutput: {
      hookEventName: "PostToolUse",
      additionalContext: context,
    },
  });
}

function reportFailure(label) {
  process.stderr.write(`${label} failed\n`);
}

export function runAislopHook(stdinText, bin = AISLOP_BIN, label = "aislop hook") {
  return new Promise((resolve) => {
    const child = spawn(bin, ["hook", "claude"], {
      cwd: process.cwd(),
      env: { ...process.env, AISLOP_NO_TELEMETRY: "1" },
      stdio: ["pipe", "pipe", "inherit"],
    });
    let stdout = "";
    let settled = false;

    const finish = (code) => {
      if (settled) return;
      settled = true;
      if (code !== 0) reportFailure(label);
      resolve(code);
    };

    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.on("error", () => finish(1));
    child.on("close", (code, signal) => {
      if (code !== 0 || signal !== null) {
        finish(1);
        return;
      }

      try {
        process.stdout.write(compactAislopOutput(stdout));
        finish(0);
      } catch {
        finish(1);
      }
    });
    child.stdin.on("error", () => {});
    child.stdin.end(stdinText);
  });
}
