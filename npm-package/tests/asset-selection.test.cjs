// Wrapper tests: the npm installer selects a profile/OS/arch asset, derives the
// right SHA-256 checksums URL, and names the asset explicitly on failure.
// Run with: node --test npm-package/tests/

const test = require("node:test");
const assert = require("node:assert/strict");
const os = require("node:os");
const Module = require("node:module");

// adm-zip is only needed when extracting; stub it so the tests need no install.
const originalLoad = Module._load;
Module._load = function (request, parent, isMain) {
  if (request === "adm-zip") return function AdmZip() {};
  return originalLoad.call(this, request, parent, isMain);
};
const installer = require("../install.js");
Module._load = originalLoad;

function withPlatform(type, arch, fn) {
  const originalType = os.type;
  const originalArch = os.arch;
  os.type = () => type;
  os.arch = () => arch;
  try {
    return fn();
  } finally {
    os.type = originalType;
    os.arch = originalArch;
  }
}

function withProfile(profile, fn) {
  const before = process.env.BASEMIND_PROFILE;
  if (profile === undefined) delete process.env.BASEMIND_PROFILE;
  else process.env.BASEMIND_PROFILE = profile;
  try {
    return fn();
  } finally {
    if (before === undefined) delete process.env.BASEMIND_PROFILE;
    else process.env.BASEMIND_PROFILE = before;
  }
}

test("legacy default resolves the full profile asset", () => {
  withPlatform("Linux", "x64", () => {
    withProfile(undefined, () => {
      const assets = installer.getReleaseAssets();
      assert.equal(assets.profile, "full");
      assert.equal(assets.assetName, "basemind-x86_64-unknown-linux-gnu.tar.gz");
      assert.match(assets.archiveUrl, /basemind-x86_64-unknown-linux-gnu\.tar\.gz$/u);
      assert.match(assets.checksumsUrl, /basemind_\d+\.\d+\.\d+_checksums\.txt$/u);
    });
  });
});

test("explicit hacienda-legal choice resolves the confidential asset", () => {
  withPlatform("Linux", "x64", () => {
    withProfile("hacienda-legal", () => {
      const assets = installer.getReleaseAssets();
      assert.equal(assets.profile, "hacienda-legal");
      assert.equal(assets.assetName, "basemind-hacienda-legal-x86_64-unknown-linux-gnu.tar.gz");
    });
  });
});

test("Windows uses .zip for both profiles", () => {
  withPlatform("Windows_NT", "x64", () => {
    for (const [profile, expected] of [
      ["full", "basemind-x86_64-pc-windows-msvc.zip"],
      ["hacienda-legal", "basemind-hacienda-legal-x86_64-pc-windows-msvc.zip"],
    ]) {
      withProfile(profile, () => {
        assert.equal(installer.getReleaseAssets().assetName, expected);
      });
    }
  });
});

test("unknown profile fails before any download", () => {
  withProfile("legal", () => {
    assert.throws(() => installer.getReleaseAssets(), /unknown BASEMIND_PROFILE 'legal'/u);
  });
});

test("checksums file distinguishes the two profiles at one version", () => {
  const sums = [
    "aaaa  basemind-x86_64-unknown-linux-gnu.tar.gz",
    "bbbb  basemind-hacienda-legal-x86_64-unknown-linux-gnu.tar.gz",
  ].join("\n");
  assert.equal(installer.expectedDigest(sums, "basemind-hacienda-legal-x86_64-unknown-linux-gnu.tar.gz"), "bbbb");
  assert.equal(installer.expectedDigest(sums, "basemind-x86_64-unknown-linux-gnu.tar.gz"), "aaaa");
  assert.equal(installer.expectedDigest(sums, "basemind-hacienda-legal-aarch64-apple-darwin.tar.gz"), null);
});

// Goldziher/basemind is the canonical release source: the repository's upstream
// remote, Cargo/pyproject metadata, plugin launcher and Homebrew tap all name it.
// Pin the wrapper to it so a regression to the fork's host is an explicit change.
test("release URL uses the canonical Goldziher upstream", () => {
  withPlatform("Linux", "x64", () => {
    withProfile("full", () => {
      const assets = installer.getReleaseAssets();
      assert.match(assets.archiveUrl, /^https:\/\/github\.com\/Goldziher\/basemind\/releases\/download\//u);
      assert.match(assets.releaseUrl, /^https:\/\/github\.com\/Goldziher\/basemind\/releases\/tag\//u);
    });
  });
});

test("wrapper source contains no legacy jamon8888 release URL", () => {
  const source = require("node:fs").readFileSync(require.resolve("../install.js"), "utf8");
  assert.doesNotMatch(source, /jamon8888\/basemind\/releases/u);
});
