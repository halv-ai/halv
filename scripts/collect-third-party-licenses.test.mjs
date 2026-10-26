import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  EXTRA_LICENSE_TEXTS,
  LICENSE_TEXTS_DIR,
  buildLicenseReport,
  collectCargoPackages,
  findLicenseFiles,
  isMainModule,
  loadLicenseTexts,
  parseCargoTree,
  parseSpdxExpression,
  runCollector,
  selectStandardLicenses,
} from "./collect-third-party-licenses.mjs";

const SCRIPTS_DIR = dirname(fileURLToPath(import.meta.url));
const COLLECTOR_PATH = join(SCRIPTS_DIR, "collect-third-party-licenses.mjs");
const REGISTRY = "registry+https://github.com/rust-lang/crates.io-index";
const VENDORED_LICENSES = [
  "0BSD",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "BSL-1.0",
  "CC0-1.0",
  "CDLA-Permissive-2.0",
  "ISC",
  "MIT",
  "MPL-2.0",
  "Unicode-3.0",
  "Unlicense",
  "Zlib",
];

function writeFiles(directory, files) {
  mkdirSync(directory, { recursive: true });
  for (const [name, text] of Object.entries(files)) {
    mkdirSync(join(directory, name, ".."), { recursive: true });
    writeFileSync(join(directory, name), text);
  }
  return directory;
}

function cargoPackage(root, name, version, files, fields = {}) {
  const directory = writeFiles(join(root, "cargo", `${name}-${version}`), {
    "Cargo.toml": `[package]\nname = "${name}"\n`,
    ...files,
  });
  return {
    id: `${REGISTRY}#${name}@${version}`,
    name,
    version,
    source: REGISTRY,
    manifest_path: join(directory, "Cargo.toml"),
    license: null,
    license_file: null,
    repository: `https://example.com/${name}`,
    homepage: null,
    authors: [],
    ...fields,
  };
}

function npmPackage(root, name, version, files, manifest = {}) {
  return writeFiles(join(root, "npm", `${name}@${version}`), {
    "package.json": JSON.stringify({ name, version, ...manifest }),
    ...files,
  });
}

function edge(pkg, ...kinds) {
  return { pkg: pkg.id, dep_kinds: kinds.map((kind) => ({ kind, target: null })) };
}

// Gives cargo tree output for the packages, in the
// `cargo tree --prefix none --format {p}` format.
function cargoTree(packages, suffixes = {}) {
  return `${packages
    .map((pkg) => `${pkg.name} v${pkg.version}${suffixes[pkg.name] ?? ""}`)
    .join("\n")}\n`;
}

// Builds a fixture tree: one Cargo workspace with its cargo tree output, one
// pnpm tree, the devDependency list for the build injected packages, and the
// extra license texts.
function createFixture() {
  const root = mkdtempSync(join(tmpdir(), "halv-third-party-licenses-"));
  const app = {
    id: "path+file:///app#app@1.0.0",
    name: "app",
    version: "1.0.0",
    source: null,
    manifest_path: join(root, "app", "Cargo.toml"),
    license: "Apache-2.0",
  };
  const withFile = cargoPackage(root, "alpha", "1.0.0", {
    LICENSE: "Alpha license\r\nCopyright Alpha authors   \r\n",
  }, { license: "MIT" });
  const sameText = cargoPackage(root, "alpha-twin", "1.0.0", {
    LICENSE: "Alpha license\nCopyright Alpha authors\n",
  }, { license: "MIT" });
  const expressionOnly = cargoPackage(root, "beta", "2.0.0", {}, {
    license: "MIT",
    authors: ["Beta Dev <beta@example.com>", "Beta Two"],
  });
  const dualWithFiles = cargoPackage(root, "gamma", "0.1.0", {
    "LICENSE-APACHE": "Gamma Apache text\n",
    "LICENSE-MIT": "Gamma MIT text\n",
    "README.md": "not a license\n",
  }, { license: "MIT OR Apache-2.0" });
  const dualExpressionOnly = cargoPackage(root, "delta", "0.3.0", {}, {
    license: "MIT/Apache-2.0",
  });
  const declaredFile = cargoPackage(root, "epsilon", "1.0.0", {
    "docs/TERMS.txt": "Epsilon terms\n",
  }, { license_file: "docs/TERMS.txt" });
  const buildOnly = cargoPackage(root, "build-helper", "1.0.0", {
    "LICENSE.txt": "Build helper license\n",
  }, { license: "MIT" });
  const devOnly = cargoPackage(root, "dev-helper", "1.0.0", {
    LICENSE: "Dev helper license\n",
  }, { license: "MIT" });
  // cargo metadata reaches this crate through a feature that a release
  // build does not turn on, so cargo tree does not name it.
  const metadataOnly = cargoPackage(root, "unused-feature", "1.0.0", {
    LICENSE: "Unused feature license\n",
  }, { license: "MIT" });
  const vendoredCode = cargoPackage(root, "zeta-sys", "0.2.0", {
    "LICENSE-MIT": "Zeta MIT text\n",
    "vendor/libzeta/COPYING": "libzeta license\n",
    "vendor/libzeta/src/copying.c": "not a license\n",
    "src/copying.rs": "not a license\n",
    "tests/fixtures/LICENSE": "test fixture license\n",
    "a/b/c/d/LICENSE": "deepest scanned license\n",
    "a/b/c/d/e/LICENSE": "too deep license\n",
    "deep/path/beyond/scan/depth/LICENSE-DATA": "mapped data license\n",
    COPYRIGHT: "Zeta copyright notice\n",
  }, { license: "MIT" });
  const noFileUpstream = cargoPackage(root, "eta", "1.1.0", {}, { license: "MIT" });
  const packages = [
    app,
    withFile,
    sameText,
    expressionOnly,
    dualWithFiles,
    dualExpressionOnly,
    declaredFile,
    buildOnly,
    devOnly,
    metadataOnly,
    vendoredCode,
    noFileUpstream,
  ];
  const shipped = [
    app,
    withFile,
    sameText,
    expressionOnly,
    dualWithFiles,
    dualExpressionOnly,
    declaredFile,
    buildOnly,
    vendoredCode,
    noFileUpstream,
  ];
  const metadata = {
    packages,
    workspace_members: [app.id],
    resolve: {
      root: app.id,
      nodes: [
        {
          id: app.id,
          deps: [
            edge(withFile, "dev", null),
            edge(expressionOnly, null),
            edge(dualWithFiles, null),
            edge(dualExpressionOnly, null),
            edge(declaredFile, null),
            edge(buildOnly, "build"),
            edge(devOnly, "dev"),
            edge(metadataOnly, null),
            edge(vendoredCode, null),
            edge(noFileUpstream, null),
          ],
        },
        { id: withFile.id, deps: [edge(sameText, null)] },
        ...packages.slice(2).map((pkg) => ({ id: pkg.id, deps: [] })),
      ],
    },
  };
  const tree = cargoTree(shipped, {
    app: " (/app)",
    "build-helper": " (proc-macro)",
    "alpha-twin": " (*)",
  });

  const vendoredDir = writeFiles(join(root, "license-texts"), {
    "packages/eta/LICENSE": "MIT License\n\nCopyright (c) 2020 Eta Upstream\n",
  });
  const extraTexts = {
    "cargo:eta": {
      note: "The published package has no license file.",
      texts: [{ vendored: "packages/eta/LICENSE", role: "license" }],
    },
    "cargo:zeta-sys": {
      note: "The crate compiles libzeta.",
      texts: [
        { file: "vendor/libzeta/COPYING", role: "component" },
        { file: "deep/path/beyond/scan/depth/LICENSE-DATA", role: "component" },
      ],
    },
  };

  const react = npmPackage(root, "react", "18.3.1", { LICENSE: "React MIT text\n" }, {
    license: "MIT",
    repository: { type: "git", url: "git+https://github.com/facebook/react.git" },
  });
  const looseEnvify = npmPackage(root, "loose-envify", "1.4.0", {
    "license.md": "Loose envify text\n",
  }, { license: "MIT", repository: "zertosh/loose-envify" });
  const reactDom = npmPackage(root, "react-dom", "18.3.1", { LICENSE: "React MIT text\n" }, {
    license: "MIT",
  });
  const vite = npmPackage(root, "vite", "6.4.4", { "LICENSE.md": "Vite text\n" }, {
    license: "MIT",
  });
  const esbuild = npmPackage(root, "esbuild", "0.25.12", { "LICENSE.md": "esbuild text\n" }, {
    license: "MIT",
  });
  const spdxOnly = npmPackage(root, "@scope/plugin", "2.0.0", {
    "LICENSE.spdx": "SPDXVersion: SPDX-2.1\nPackageCopyrightText: Plugin authors\n",
  }, { license: "MIT OR Apache-2.0" });
  const pnpmProduction = [
    {
      name: "app",
      dependencies: {
        "@scope/plugin": { version: "2.0.0", path: spdxOnly },
        react: {
          version: "18.3.1",
          path: react,
          dependencies: {
            "loose-envify": { version: "1.4.0", path: looseEnvify, deduped: true },
          },
        },
        "react-dom": {
          version: "18.3.1",
          path: reactDom,
          dependencies: { "loose-envify": { version: "1.4.0", path: looseEnvify } },
        },
      },
    },
  ];
  const pnpmDevelopment = [
    {
      name: "app",
      devDependencies: {
        vite: {
          version: "6.4.4",
          path: vite,
          dependencies: { esbuild: { version: "0.25.12", path: esbuild } },
        },
      },
    },
  ];
  return {
    root,
    vendoredDir,
    packages: { metadataOnly, noFileUpstream, vendoredCode },
    inputs: { cargo: [{ metadata, tree }], pnpmProduction, pnpmDevelopment },
    buildInjected: {
      esbuild: { parent: "vite", note: "Vite runs esbuild." },
      vite: { note: "The build copies Vite code." },
    },
    extraTexts,
  };
}

function entry(markdown, name) {
  const start = markdown.indexOf(`\n### ${name}\n`);
  assert.notEqual(start, -1, `no entry for ${name}`);
  return markdown.slice(start + 1, markdown.indexOf("\n### ", start + 1));
}

function report(fixture, exceptions = []) {
  return buildLicenseReport({
    ...fixture.inputs,
    exceptions,
    licenseTexts: loadLicenseTexts(LICENSE_TEXTS_DIR),
    buildInjected: fixture.buildInjected,
    extraTexts: fixture.extraTexts,
    licenseTextsDir: fixture.vendoredDir,
  });
}

// Adds a crate that the workspace reaches with a normal edge and that cargo
// tree names.
function addShippedCrate(fixture, pkg) {
  const { metadata } = fixture.inputs.cargo[0];
  metadata.packages.push(pkg);
  metadata.resolve.nodes.push({ id: pkg.id, deps: [] });
  metadata.resolve.nodes[0].deps.push(edge(pkg, null));
  fixture.inputs.cargo[0].tree += `${pkg.name} v${pkg.version}\n`;
}

function withFixture(run) {
  const fixture = createFixture();
  try {
    run(fixture);
  } finally {
    rmSync(fixture.root, { recursive: true, force: true });
  }
}

test("vendors a standard text for each listed SPDX license", () => {
  const texts = loadLicenseTexts(LICENSE_TEXTS_DIR);
  assert.deepEqual([...texts.keys()].sort(), [...VENDORED_LICENSES].sort());
  assert.match(texts.get("MIT"), /^MIT License\n/u);
  assert.match(texts.get("Apache-2.0"), /Apache License\s+Version 2\.0/u);
  assert.match(texts.get("Zlib"), /^zlib License\n/u);
});

test("each extra license text of the real configuration exists", () => {
  for (const [key, extra] of Object.entries(EXTRA_LICENSE_TEXTS)) {
    assert.ok(extra.note.trim(), `${key} has no note`);
    for (const text of extra.texts) {
      if (text.vendored) {
        const content = readFileSync(join(LICENSE_TEXTS_DIR, text.vendored), "utf8");
        assert.ok(content.trim(), `${key}: ${text.vendored} is empty`);
      }
    }
  }
});

test("parses SPDX expressions and selects standard texts", () => {
  const available = new Set(["MIT", "Apache-2.0", "Unicode-3.0", "Zlib", "ISC"]);
  const templated = new Set(["MIT", "ISC"]);
  const select = (expression, templates = new Set()) =>
    selectStandardLicenses(parseSpdxExpression(expression), available, templates);
  assert.deepEqual(select("MIT"), ["MIT"]);
  assert.deepEqual(select("MIT", templated), ["MIT"]);
  assert.deepEqual(select("Zlib OR Apache-2.0 OR MIT"), ["Zlib"]);
  assert.deepEqual(select("BSD-1-Clause OR Apache-2.0 OR MIT"), ["Apache-2.0"]);
  assert.deepEqual(select("MIT/Apache-2.0"), ["MIT"]);
  // A choice without a copyright line template wins over the written order.
  assert.deepEqual(select("MIT/Apache-2.0", templated), ["Apache-2.0"]);
  assert.deepEqual(select("ISC OR MIT", templated), ["ISC"]);
  assert.deepEqual(select("(MIT OR Apache-2.0) AND Unicode-3.0"), ["MIT", "Unicode-3.0"]);
  assert.deepEqual(
    select("(MIT OR Apache-2.0) AND Unicode-3.0", templated),
    ["Apache-2.0", "Unicode-3.0"],
  );
  assert.deepEqual(select("Apache-2.0 WITH LLVM-exception OR MIT"), ["MIT"]);
  assert.equal(select("Apache-2.0 WITH LLVM-exception"), null);
  assert.equal(select("MIT AND BSD-1-Clause"), null);
  assert.throws(() => parseSpdxExpression("MIT OR"), /invalid SPDX/u);
  assert.throws(() => parseSpdxExpression("(MIT"), /invalid SPDX/u);
});

test("parses cargo tree output", () => {
  const keys = parseCargoTree(
    "halv v1.0.1 (/Users/dev/halv/src-tauri)\n" +
      "tauri v2.11.5\n" +
      "serde_derive v1.0.228 (proc-macro)\n" +
      "serde_derive v1.0.228 (proc-macro) (*)\n" +
      "wasip2 v1.0.4+wasi-0.2.12 (*)\n" +
      "gitdep v0.1.0 (https://github.com/example/gitdep?rev=abc#abc)\r\n" +
      "winlocal v0.2.0 (C:\\Program Files (x86)\\src\\winlocal) (*)\r\n" +
      "\n",
  );
  assert.deepEqual([...keys].sort(), [
    "gitdep@0.1.0",
    "halv@1.0.1",
    "serde_derive@1.0.228",
    "tauri@2.11.5",
    "wasip2@1.0.4+wasi-0.2.12",
    "winlocal@0.2.0",
  ]);
  assert.throws(() => parseCargoTree("[build-dependencies]\n"), /unexpected cargo tree line/u);
  assert.throws(() => parseCargoTree("tauri 2.11.5\n"), /unexpected cargo tree line/u);
  assert.throws(() => parseCargoTree("\n"), /printed no packages/u);
});

test("lists the crates that cargo tree names and the metadata graph reaches", () => {
  withFixture((fixture) => {
    const { metadata, tree } = fixture.inputs.cargo[0];
    const names = collectCargoPackages(metadata, tree)
      .map(({ name }) => name)
      .sort();
    assert.deepEqual(names, [
      "alpha",
      "alpha-twin",
      "beta",
      "build-helper",
      "delta",
      "epsilon",
      "eta",
      "gamma",
      "zeta-sys",
    ]);
    // cargo tree must not name a crate that only a dev-dependency reaches.
    assert.throws(
      () => collectCargoPackages(metadata, `${tree}dev-helper v1.0.0\n`),
      /cargo tree names dev-helper@1\.0\.0, but the normal and build edges/u,
    );
  });
});

test("finds license files, notices, and license files of vendored code", () => {
  withFixture((fixture) => {
    const directory = dirname(fixture.packages.vendoredCode.manifest_path);
    const files = findLicenseFiles(directory).map(({ name, role }) => `${role} ${name}`);
    // The order is by code point, so upper case names come first.
    assert.deepEqual(files, [
      "notice COPYRIGHT",
      "license LICENSE-MIT",
      "component a/b/c/d/LICENSE",
      "component vendor/libzeta/COPYING",
    ]);
  });
});

test("copies license files and uses standard texts only as a fallback", () => {
  withFixture((fixture) => {
    const { markdown, counts, errors } = report(fixture);
    assert.deepEqual(errors, []);
    assert.deepEqual(counts, {
      cargo: 9,
      npm: 6,
      standard: 3,
      exceptions: 0,
      texts: 18,
    });

    // A crate with a license file: the file text, without its CR and trailing spaces.
    assert.match(entry(markdown, "alpha 1.0.0"), /- License: MIT\n/u);
    assert.match(entry(markdown, "alpha 1.0.0"), /\(`LICENSE`\)/u);
    assert.match(markdown, /```text\nAlpha license\nCopyright Alpha authors\n```/u);
    // Equal texts appear one time.
    assert.equal(markdown.split("Alpha license\n").length - 1, 1);

    // A crate with only an MIT expression: the standard text with the package
    // authors in the copyright line.
    const beta = entry(markdown, "beta 2.0.0");
    assert.match(beta, /\(standard SPDX text: MIT, copyright line from the package authors\)/u);
    assert.match(beta, /- Authors: Beta Dev, Beta Two\n/u);
    assert.doesNotMatch(markdown, /beta@example\.com/u);
    assert.match(
      markdown,
      /Standard SPDX text: MIT\. The copyright line names the package authors\.\n\n```text\nMIT License\n\nCopyright \(c\) Beta Dev, Beta Two\n/u,
    );
    assert.doesNotMatch(markdown, /<copyright holders>/u);

    // A dual license crate with both files: each file.
    const gamma = entry(markdown, "gamma 0.1.0");
    assert.match(gamma, /- License: MIT OR Apache-2\.0\n/u);
    assert.match(gamma, /\(`LICENSE-APACHE`\), \[t-[0-9a-f]{12}\]\(#t-[0-9a-f]{12}\) \(`LICENSE-MIT`\)/u);
    assert.doesNotMatch(markdown, /not a license/u);

    // A dual license crate without files and authors: the choice whose text
    // has no copyright line template.
    const delta = entry(markdown, "delta 0.3.0");
    assert.match(delta, /- License: MIT OR Apache-2\.0\n/u);
    assert.match(delta, /\(standard SPDX text: Apache-2\.0\)/u);
    assert.match(markdown, /Standard SPDX text: Apache-2\.0\n\n```text\n/u);

    // The license file that the metadata declares.
    assert.match(entry(markdown, "epsilon 1.0.0"), /\(`docs\/TERMS\.txt`\)/u);
    assert.match(entry(markdown, "build-helper 1.0.0"), /\(`LICENSE\.txt`\)/u);
    assert.doesNotMatch(markdown, /dev-helper/u);
    assert.doesNotMatch(markdown, /unused-feature/u);
  });
});

test("adds the extra license texts and the license files of vendored code", () => {
  withFixture((fixture) => {
    const { markdown, errors } = report(fixture);
    assert.deepEqual(errors, []);

    const zeta = entry(markdown, "zeta-sys 0.2.0");
    assert.match(zeta, /- Note: The crate compiles libzeta\.\n/u);
    for (const label of [
      "`COPYRIGHT`",
      "`LICENSE-MIT`",
      "`a/b/c/d/LICENSE`",
      "`vendor/libzeta/COPYING`",
      "`deep/path/beyond/scan/depth/LICENSE-DATA`",
    ]) {
      assert.ok(zeta.includes(`(${label})`), `zeta-sys does not name ${label}`);
    }
    // The extra file and the scan find the same file. The entry names it one time.
    assert.equal(zeta.split("vendor/libzeta/COPYING").length - 1, 1);
    assert.doesNotMatch(markdown, /too deep license|test fixture license/u);
    assert.match(markdown, /```text\nlibzeta license\n```/u);

    // A vendored copy of the upstream license replaces the standard text.
    const eta = entry(markdown, "eta 1.1.0");
    assert.match(eta, /\(copy of the upstream text: `packages\/eta\/LICENSE`\)/u);
    assert.doesNotMatch(eta, /standard SPDX text/u);
    assert.match(markdown, /Copyright \(c\) 2020 Eta Upstream/u);
  });
});

test("fails for an extra license text that does not exist or matches no package", () => {
  withFixture((fixture) => {
    fixture.extraTexts["cargo:zeta-sys"].texts.push({
      file: "vendor/missing/COPYING",
      role: "component",
    });
    assert.throws(
      () => report(fixture),
      /cargo:zeta-sys@0\.2\.0: the extra license text vendor\/missing\/COPYING does not exist/u,
    );
  });
  withFixture((fixture) => {
    fixture.extraTexts["cargo:eta"].texts[0].vendored = "packages/eta/MISSING";
    assert.throws(() => report(fixture), /extra license text packages\/eta\/MISSING does not exist/u);
  });
  withFixture((fixture) => {
    fixture.extraTexts["cargo:gone"] = {
      note: "Old entry.",
      texts: [{ file: "LICENSE", role: "license" }],
    };
    fixture.extraTexts["cargo:zeta-sys"].versions = ["0.1.0"];
    const { errors } = report(fixture);
    assert.deepEqual(errors, [
      'cargo:zeta-sys@0.2.0 is not in the versions of EXTRA_LICENSE_TEXTS "cargo:zeta-sys" ' +
        "(0.1.0). Check the license texts of this version and update the entry.",
      'EXTRA_LICENSE_TEXTS "cargo:gone" matches no package. Remove it.',
    ]);
  });
  withFixture((fixture) => {
    fixture.extraTexts["cargo:eta"] = { note: "", texts: [] };
    assert.throws(() => report(fixture), /extra license texts cargo:eta need/u);
  });
});

test("needs package authors or an upstream text for a standard text with a template", () => {
  withFixture((fixture) => {
    delete fixture.extraTexts["cargo:eta"];
    const { errors } = report(fixture);
    assert.equal(errors.length, 1);
    assert.match(
      errors[0],
      /^cargo:eta@1\.1\.0 has no license file, and the standard MIT text needs a copyright holder/u,
    );
  });
});

test("lists npm packages and the build injected packages", () => {
  withFixture((fixture) => {
    const { markdown } = report(fixture);
    const npmPart = markdown.slice(markdown.indexOf("\n## npm packages\n"));
    assert.match(npmPart, /^### loose-envify 1\.4\.0$/mu);
    assert.match(npmPart, /^### react-dom 18\.3\.1$/mu);
    const react = entry(markdown, "react 18.3.1");
    assert.match(react, /- Source: https:\/\/github\.com\/facebook\/react\n/u);
    assert.match(react, /\(`LICENSE`\)/u);
    assert.match(
      entry(markdown, "loose-envify 1.4.0"),
      /- Source: https:\/\/github\.com\/zertosh\/loose-envify\n/u,
    );
    const vite = entry(markdown, "vite 6.4.4");
    assert.match(vite, /- Note: The build copies Vite code\.\n/u);
    assert.match(vite, /\(`LICENSE\.md`\)/u);
    // A build injected package that a devDependency depends on.
    const esbuild = entry(markdown, "esbuild 0.25.12");
    assert.match(esbuild, /- Note: Vite runs esbuild\.\n/u);
    assert.match(esbuild, /\(`LICENSE\.md`\)/u);
    // A LICENSE.spdx file holds metadata, so the entry also gets the standard text.
    const plugin = entry(markdown, "@scope/plugin 2.0.0");
    assert.match(plugin, /\(`LICENSE\.spdx`\), \[t-[0-9a-f]{12}\]\(#t-[0-9a-f]{12}\) \(standard SPDX text: Apache-2\.0\)/u);
  });
  withFixture((fixture) => {
    fixture.buildInjected.rollup = { parent: "vite", note: "Vite runs Rollup." };
    assert.throws(() => report(fixture), /pnpm list --dev gave no path for vite > rollup/u);
  });
});

test("names a text one time when two files of a package hold it", () => {
  withFixture((fixture) => {
    addShippedCrate(
      fixture,
      cargoPackage(fixture.root, "theta", "1.0.0", {
        LICENSE: "Theta license\n",
        "lib/css/LICENSE": "Normalize license\n",
        "src/css/LICENSE": "Normalize license\n",
      }, { license: "MIT" }),
    );
    const theta = entry(report(fixture).markdown, "theta 1.0.0");
    assert.match(theta, /\(`lib\/css\/LICENSE`, `src\/css\/LICENSE`\)/u);
  });
});

test("needs an exception with a reason for a package without license text", () => {
  withFixture((fixture) => {
    addShippedCrate(
      fixture,
      cargoPackage(fixture.root, "bare", "0.1.0", {}, { license: "LicenseRef-Custom" }),
    );

    const missing = report(fixture);
    assert.equal(missing.errors.length, 1);
    assert.match(missing.errors[0], /^cargo:bare@0\.1\.0 has no license file/u);

    const noReason = report(fixture, [
      { ecosystem: "cargo", name: "bare", version: "0.1.0", reason: " " },
    ]);
    assert.match(noReason.errors.join("\n"), /exception 0 needs/u);

    const excepted = report(fixture, [
      { ecosystem: "cargo", name: "bare", version: "0.1.0", reason: "Fixture reason." },
    ]);
    assert.deepEqual(excepted.errors, []);
    assert.equal(excepted.counts.exceptions, 1);
    assert.match(
      entry(excepted.markdown, "bare 0.1.0"),
      /- License texts: none\. Reason: Fixture reason\.\n/u,
    );

    const stale = report(fixture, [
      { ecosystem: "cargo", name: "bare", version: "0.1.0", reason: "Fixture reason." },
      { ecosystem: "npm", name: "react", version: "18.3.1", reason: "Old reason." },
    ]);
    assert.deepEqual(stale.errors, [
      "exception npm:react@18.3.1 does not match a package without license text. Remove it.",
    ]);
  });
});

test("keeps the notice files of a package without license text", () => {
  withFixture((fixture) => {
    addShippedCrate(
      fixture,
      cargoPackage(fixture.root, "noticed", "0.1.0", { NOTICE: "Noticed notice\n" }, {
        license: "LicenseRef-Custom",
      }),
    );
    const excepted = report(fixture, [
      { ecosystem: "cargo", name: "noticed", version: "0.1.0", reason: "Fixture reason." },
    ]);
    assert.deepEqual(excepted.errors, []);
    assert.match(
      entry(excepted.markdown, "noticed 0.1.0"),
      /- License texts: \[t-[0-9a-f]{12}\]\(#t-[0-9a-f]{12}\) \(`NOTICE`\)\. None of them is the license of the package\. Reason: Fixture reason\.\n/u,
    );
  });
});

test("finds THIRD_PARTY files as notices, with the same exclusions as other license files", () => {
  withFixture((fixture) => {
    const directory = writeFiles(join(fixture.root, "third-party-names"), {
      "THIRD_PARTY": "Apple documentation license\n",
      "THIRD-PARTY-NOTICES.md": "Dash notices\n",
      "ThirdPartyNotices.txt": "Camel case notices\n",
      "LICENSE": "Package license\n",
      "src/THIRD_PARTY": "Nested third party notices\n",
      // A code file holds code, not a license text.
      "third_party.rs": "pub fn third_party() {}\n",
      "third_party.d.ts": "export declare const thirdParty: string;\n",
      // The scan does not go into folders for tests, examples, and benches.
      "tests/THIRD_PARTY": "test notices\n",
      "examples/ThirdPartyNotices.txt": "example notices\n",
      "benches/THIRD-PARTY-NOTICES.md": "bench notices\n",
      // The name must start with the third party words.
      "about-third-party.md": "not a license file\n",
      "README.md": "not a license file\n",
    });
    const files = findLicenseFiles(directory).map(({ name, role }) => `${role} ${name}`);
    // The order is by code point, so upper case names come first.
    assert.deepEqual(files, [
      "license LICENSE",
      "notice THIRD-PARTY-NOTICES.md",
      "notice THIRD_PARTY",
      "notice ThirdPartyNotices.txt",
      "component src/THIRD_PARTY",
    ]);
  });
});

test("names a THIRD_PARTY file in the entry, but does not use it as the package license", () => {
  withFixture((fixture) => {
    addShippedCrate(
      fixture,
      cargoPackage(fixture.root, "thirdparty-only", "0.1.0", {
        THIRD_PARTY: "Adapted documentation license\n",
      }, { license: "MIT", authors: ["Third Party Dev"] }),
    );
    addShippedCrate(
      fixture,
      cargoPackage(fixture.root, "thirdparty-dual", "0.2.0", {
        "LICENSE-APACHE": "Dual Apache text\n",
        "LICENSE-MIT": "Dual MIT text\n",
        THIRD_PARTY: "Dual adapted documentation license\n",
      }, { license: "MIT OR Apache-2.0" }),
    );
    const { markdown, errors } = report(fixture);
    assert.deepEqual(errors, []);

    // The crate has no license file. The entry names the THIRD_PARTY text and
    // also uses the standard MIT text for its license.
    const only = entry(markdown, "thirdparty-only 0.1.0");
    assert.match(only, /\(`THIRD_PARTY`\)/u);
    assert.match(only, /\(standard SPDX text: MIT, copyright line from the package authors\)/u);
    assert.match(markdown, /```text\nAdapted documentation license\n```/u);

    // The crate has license files. The entry names them and the THIRD_PARTY
    // text, and it has no standard text.
    const dual = entry(markdown, "thirdparty-dual 0.2.0");
    for (const label of ["`LICENSE-APACHE`", "`LICENSE-MIT`", "`THIRD_PARTY`"]) {
      assert.ok(dual.includes(`(${label})`), `thirdparty-dual does not name ${label}`);
    }
    assert.doesNotMatch(dual, /standard SPDX text/u);
    assert.match(markdown, /```text\nDual adapted documentation license\n```/u);
  });
});

test("the header names the files that the license list covers", () => {
  withFixture((fixture) => {
    const { markdown, errors } = report(fixture);
    assert.deepEqual(errors, []);
    // The header wraps its lines, so the checks use one space between words.
    const header = markdown.slice(0, markdown.indexOf("\n## ")).replace(/\s+/gu, " ");
    // The list covers the crates of the app and of the supervisor sidecar, and
    // the npm packages of the frontend.
    assert.match(header, /that the Halv app contains/u);
    assert.match(header, /`src-tauri`/u);
    assert.match(header, /`halv-supervisor`/u);
    assert.match(header, /`src-tauri\/supervisor`/u);
    // The list does not cover the crates of the RTK and Crux binaries.
    assert.match(header, /RTK and Crux binaries/u);
    assert.match(header, /does not list those crates/u);
    assert.match(header, /`THIRD_PARTY_NOTICES\.md`/u);
    // The header does not claim that the list covers each installer part.
    assert.doesNotMatch(header, /installer can contain/u);
  });
});

test("--check exits 1 when the committed file differs from a fresh run", () => {
  withFixture((fixture) => {
    const messages = [];
    const outputPath = join(fixture.root, "THIRD_PARTY_LICENSES.md");
    const exceptionsPath = join(fixture.root, "exceptions.json");
    writeFileSync(exceptionsPath, JSON.stringify({ exceptions: [] }));
    const run = (argv) =>
      runCollector({
        argv,
        appRoot: fixture.root,
        outputPath,
        exceptionsPath,
        inputs: () => fixture.inputs,
        buildInjected: fixture.buildInjected,
        extraTexts: fixture.extraTexts,
        licenseTextsDir: fixture.vendoredDir,
        log: (message) => messages.push(message),
        error: (message) => messages.push(message),
      });
    // The fixture license text folder has no standard texts, so the fixture
    // run reads them from a copy.
    for (const [id, text] of loadLicenseTexts(LICENSE_TEXTS_DIR)) {
      writeFileSync(join(fixture.vendoredDir, `${id}.txt`), `${text}\n`);
    }

    assert.equal(run(["--check"]), 1);
    assert.match(messages.at(-1), /is not current \(the file does not exist\)/u);

    assert.equal(run([]), 0);
    assert.match(messages.at(-1), /wrote THIRD_PARTY_LICENSES\.md: 9 Rust crates, 6 npm packages/u);
    assert.equal(run(["--check"]), 0);
    assert.match(messages.at(-1), /is current/u);

    // A file with CRLF line endings from a Windows checkout is still current.
    const written = readFileSync(outputPath, "utf8");
    writeFileSync(outputPath, written.replace(/\n/gu, "\r\n"));
    assert.equal(run(["--check"]), 0);

    writeFileSync(outputPath, written.replace("### beta 2.0.0", "### beta 2.0.1"));
    assert.equal(run(["--check"]), 1);
    assert.match(messages.at(-1), /is not current \(first difference at line \d+\)/u);

    assert.equal(run(["--bogus"]), 2);
  });
});

test("runs when Node starts the script through a symlink", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-third-party-licenses-link-"));
  try {
    const linkedScripts = join(root, "scripts");
    // A junction needs no special right on Windows. Other platforms ignore the type.
    symlinkSync(SCRIPTS_DIR, linkedScripts, "junction");
    const linkedCollector = join(linkedScripts, "collect-third-party-licenses.mjs");
    const collectorUrl = pathToFileURL(realpathSync(COLLECTOR_PATH)).href;

    // The fallback for Node versions without import.meta.main.
    assert.equal(isMainModule(collectorUrl, undefined, linkedCollector), true);
    assert.equal(isMainModule(collectorUrl, undefined, COLLECTOR_PATH), true);
    assert.equal(isMainModule(collectorUrl, undefined, join(root, "other.mjs")), false);
    assert.equal(isMainModule(collectorUrl, undefined, undefined), false);
    assert.equal(isMainModule(collectorUrl, false, COLLECTOR_PATH), false);

    const result = spawnSync(process.execPath, [linkedCollector, "--bogus"], {
      encoding: "utf8",
    });
    assert.equal(result.status, 2, result.stderr);
    assert.match(result.stderr, /unknown argument: --bogus/u);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
