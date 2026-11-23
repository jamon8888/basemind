const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const https = require("node:https");
const http = require("node:http");
const crypto = require("node:crypto");
const { execFileSync } = require("node:child_process");
const AdmZip = require("adm-zip");

const { version } = require("./package.json");

/// Distribution profiles. `full` is the default for every legacy channel name;
/// `hacienda-legal` is the confidential profile (documents + memory + NER +
/// GraphRAG, no code/comms/crawl tooling). The profile is an explicit choice via
/// BASEMIND_PROFILE; legacy channel names always install `full`.
const PROFILES = ["full", "hacienda-legal"];

function getProfile() {
  const profile = process.env.BASEMIND_PROFILE || "full";
  if (!PROFILES.includes(profile)) {
    throw new Error(
      `unknown BASEMIND_PROFILE '${profile}' — choose one of: ${PROFILES.join(", ")} ('full' is the default)`,
    );
  }
  return profile;
}

/// Release asset prefix. The full profile keeps the historical unprefixed name;
/// the confidential profile uses `basemind-hacienda-legal-<triple>`.
function assetPrefix(profile) {
  return profile === "full" ? "basemind" : `basemind-${profile}`;
}

/// One `sysctl` inquiry; `-n` prints the value of `name`, or "" when it is absent.
function sysctl(name) {
  try {
    return execFileSync("sysctl", ["-n", name], { encoding: "utf8" }).trim();
  } catch {
    return "";
  }
}

function isAppleSilicon() {
  if (os.type() !== "Darwin") return false;
  if (os.arch() === "arm64") return true;
  return sysctl("sysctl.proc_translated") === "1" || sysctl("hw.optional.arm64") === "1";
}

function getPlatformTriple() {
  const type = os.type();
  const arch = os.arch();

  if (type === "Windows_NT") {
    if (arch === "x64") return "x86_64-pc-windows-msvc";
    if (arch === "ia32") throw new Error("32-bit Windows is not supported");
  }

  if (type === "Linux") {
    if (arch === "x64") return "x86_64-unknown-linux-gnu";
    if (arch === "arm64") return "aarch64-unknown-linux-gnu";
    return "x86_64-unknown-linux-gnu";
  }

  if (type === "Darwin") {
    return isAppleSilicon() ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  }

  throw new Error(`Unsupported platform: ${type} ${arch}`);
}

function getReleaseAssets() {
  const profile = getProfile();
  const platform = getPlatformTriple();
  const baseUrl = `https://github.com/Goldziher/basemind/releases/download/v${version}`;
  const ext = platform.includes("windows") ? "zip" : "tar.gz";
  const assetName = `${assetPrefix(profile)}-${platform}.${ext}`;
  return {
    profile,
    platform,
    assetName,
    archiveUrl: `${baseUrl}/${assetName}`,
    checksumsUrl: `${baseUrl}/basemind_${version}_checksums.txt`,
    releaseUrl: `https://github.com/Goldziher/basemind/releases/tag/v${version}`,
  };
}

/// Single, explicit failure message for a missing asset or checksum. A wrapper
/// must never fall back to another profile/architecture silently.
function missingAssetError(kind, assets) {
  return new Error(
    `${kind} for profile '${assets.profile}' / ${assets.platform} at v${version} ` +
      `(${assets.assetName}). Check ${assets.releaseUrl}: archives (.tar.gz/.zip) and installers ` +
      `(.exe/.dmg) ship together at the same version, and the checksums file must list this asset.`,
  );
}

function downloadWithRedirects(url, dest, maxRedirects = 5) {
  return new Promise((resolve, reject) => {
    if (maxRedirects <= 0) {
      reject(new Error("Too many redirects"));
    }

    const urlObj = new URL(url);
    const client = urlObj.protocol === "https:" ? https : http;

    const req = client.get(
      url,
      {
        headers: {
          "User-Agent": "basemind-npm-wrapper",
        },
      },
      (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          downloadWithRedirects(res.headers.location, dest, maxRedirects - 1)
            .then(resolve)
            .catch(reject);
          return;
        }

        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode}: ${res.statusMessage}`));
          return;
        }

        const file = fs.createWriteStream(dest);
        res.pipe(file);

        file.on("finish", () => {
          file.close();
          resolve();
        });

        file.on("error", (err) => {
          fs.unlink(dest, () => {});
          reject(err);
        });
      },
    );

    req.on("error", reject);
    req.setTimeout(30000, () => {
      req.destroy();
      reject(new Error("Download timeout"));
    });
  });
}

function retryWithBackoff(fn, maxAttempts = 3) {
  const delays = [1000, 2000, 4000];
  return (async function attempt(index = 0) {
    try {
      return await fn();
    } catch (err) {
      const statusMatch = err.message.match(/HTTP ([0-9]+)/u);
      const isRetryable =
        err.message.includes("Download timeout") ||
        err.message.includes("ECONNREFUSED") ||
        err.message.includes("ECONNRESET") ||
        err.message.includes("ETIMEDOUT") ||
        err.message.includes("EHOSTUNREACH") ||
        (statusMatch && Math.trunc(Number(statusMatch[1])) >= 500);

      if (!isRetryable || index >= maxAttempts - 1) {
        throw err;
      }

      const delay = delays[index];
      console.log(`Transient error (attempt ${index + 1}/${maxAttempts}): ${err.message}; retrying in ${delay}ms...`);
      await new Promise((resolve) => {
        setTimeout(resolve, delay);
      });
      return attempt(index + 1);
    }
  })();
}

function fetchTextWithRedirects(url, maxRedirects = 5) {
  return new Promise((resolve, reject) => {
    if (maxRedirects <= 0) {
      reject(new Error("Too many redirects"));
    }

    const urlObj = new URL(url);
    const client = urlObj.protocol === "https:" ? https : http;

    const req = client.get(
      url,
      {
        headers: {
          "User-Agent": "basemind-npm-wrapper",
        },
      },
      (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          fetchTextWithRedirects(res.headers.location, maxRedirects - 1)
            .then(resolve)
            .catch(reject);
          return;
        }

        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode}: ${res.statusMessage}`));
          return;
        }

        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
        res.on("error", reject);
      },
    );

    req.on("error", reject);
    req.setTimeout(30000, () => {
      req.destroy();
      reject(new Error("Download timeout"));
    });
  });
}

function retryFetchText(url) {
  return retryWithBackoff(() => fetchTextWithRedirects(url));
}

function sha256File(filePath) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(filePath));
  return hash.digest("hex");
}

function expectedDigest(checksumsText, assetName) {
  for (const line of checksumsText.split(/\r?\n/u)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const parts = trimmed.split(/\s+/u);
    if (parts.length < 2) continue;
    const digest = parts[0];
    const name = parts.at(-1).replace(/^\*/u, "");
    if (name === assetName) return digest.toLowerCase();
  }
  return null;
}

async function verifyChecksum(archivePath, assets) {
  const { assetName, checksumsUrl } = assets;
  let checksumsText;
  try {
    checksumsText = await retryFetchText(checksumsUrl);
  } catch (error) {
    throw new Error(
      `could not fetch checksums (${checksumsUrl}): ${error.message} — refusing to install unverified binary`,
      { cause: error },
    );
  }

  const expected = expectedDigest(checksumsText, assetName);
  if (!expected) {
    throw missingAssetError(`no SHA-256 entry in ${checksumsUrl} for`, assets);
  }

  const actual = sha256File(archivePath).toLowerCase();
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${assetName} (expected ${expected}, got ${actual})`);
  }

  console.log("Checksum verified.");
}

async function installBinary() {
  try {
    const assets = getReleaseAssets();
    const { assetName, archiveUrl } = assets;
    const isZip = archiveUrl.endsWith(".zip");
    const binDir = path.join(__dirname, "bin");
    const archivePath = path.join(binDir, assetName);
    const binaryName = os.type() === "Windows_NT" ? "basemind.exe" : "basemind";
    const binaryPath = path.join(binDir, binaryName);
    const markerPath = path.join(binDir, ".profile");

    if (!fs.existsSync(binDir)) {
      fs.mkdirSync(binDir, { recursive: true });
    }

    // One binary lives in `bin/`. A marker records which profile it came from so
    // switching profiles re-downloads instead of silently running the wrong one.
    const installedProfile = fs.existsSync(markerPath) ? fs.readFileSync(markerPath, "utf8").trim() : "full";
    if (fs.existsSync(binaryPath) && installedProfile === assets.profile) {
      return;
    }

    console.log(`Downloading basemind '${assets.profile}' binary from ${archiveUrl}...`);

    try {
      await retryWithBackoff(() => downloadWithRedirects(archiveUrl, archivePath));
    } catch (error) {
      throw new Error(
        `release asset ${assetName} (profile '${assets.profile}', ${assets.platform}, v${version}) ` +
          `is missing or unreachable: ${error.message}. See ${assets.releaseUrl} — archives ` +
          `(.tar.gz/.zip) and installers (.exe/.dmg) ship together at the same version.`,
        { cause: error },
      );
    }

    await verifyChecksum(archivePath, assets);

    console.log("Extracting archive (binary + bundled libraries)...");

    if (isZip) {
      const zip = new AdmZip(archivePath);
      zip.extractAllTo(binDir, true);
    } else {
      const { extract } = await import("tar");
      await extract({
        file: archivePath,
        cwd: binDir,
      });
    }

    fs.unlinkSync(archivePath);

    if (!fs.existsSync(binaryPath)) {
      throw new Error(`binary ${binaryName} not found after extracting ${assetName}`);
    }
    fs.writeFileSync(markerPath, assets.profile);

    if (os.type() !== "Windows_NT") {
      fs.chmodSync(binaryPath, 0o755);
    }

    console.log(`basemind '${assets.profile}' binary installed successfully!`);
  } catch (error) {
    console.error("Error installing basemind binary:", error.message);
    process.exit(1);
  }
}

module.exports = {
  PROFILES,
  getProfile,
  assetPrefix,
  getPlatformTriple,
  getReleaseAssets,
  expectedDigest,
  installBinary,
};

if (require.main === module) {
  installBinary();
}
