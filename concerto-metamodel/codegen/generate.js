/*
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

'use strict';

/**
 * Regenerates `src/generated/` with concerto-codegen's Rust target.
 *
 * The four models are the vendored ASTs under `vendor/` (checked here
 * against the copies in the pinned npm packages). The Rust is written by
 * concerto-codegen's `RustVisitor`, at the version pinned in
 * `package.json` (and `package-lock.json`). `build.rs` runs this script
 * only when that version differs from the one recorded in
 * `codegen.version`, so routine builds need neither Node.js nor network.
 * After editing this file, run `npm ci && node generate.js` here.
 *
 * Drift: this script also writes `src/generated/fingerprints.tsv`, a
 * fingerprint of every input and generated file, which `tests/drift.rs`
 * checks on every `cargo test` (no Node.js needed); `node generate.js
 * --check` (run by CI) regenerates into a temporary directory and fails
 * when the result differs from `src/generated/` byte for byte.
 *
 * The wrapper this crate keeps is `MetamodelRustVisitor`: a `RustVisitor`
 * subclass that overrides `visitClassDeclaration`, `visitField`,
 * `visitEnumDeclaration` and `toRustType`, and post-processes the
 * `FileWriter` output (drops `mod.rs`, adds the `@generated` header and
 * `classes.rs`, runs rustfmt). Each override is listed below with the
 * reason the crate still needs it (accordproject/concerto-rust#461 has the
 * gap analysis):
 *
 * - `visitClassDeclaration`: an abstract type (or a concrete type with sub
 *   types that is used as a field type) is a Rust enum *named after the
 *   type*, tagged by `$class`, with a variant for every concrete type that
 *   extends it directly or not (a unit variant for the type itself when it
 *   is concrete). A struct that is only ever a variant payload has no
 *   `$class` field, since the enum tag consumes it. Fields are in model
 *   order, the root super type's first. concerto-codegen's
 *   `flattenSubclassesToUnion` lists only the direct sub types, includes
 *   abstract ones, and keeps a required `$class` on the payload structs,
 *   so none of the metamodel's unions deserialise (fixed upstream by the
 *   draft PR listed in #461; the naming and unit variant stay local, as
 *   concerto-core's source depends on them);
 * - the `$class` field is an interned `crate::ClassName` (P5-93), and
 *   `Range` and `Position` may omit it (#262);
 * - `visitField`: the `name` of a metamodel node (but an import's) is a
 *   `crate::Name` that shares the source text (P5-93), and a field with a
 *   default value (`isArray`, `isOptional`, `isAbstract`) may be absent
 *   (`#[serde(default)]`; also in the upstream draft PR);
 * - `visitEnumDeclaration`: a Concerto enum keeps the shape the published
 *   crate had before concerto-codegen generated it (`Copy`, `PartialEq`,
 *   `Eq`, PascalCase variants renamed to the value, e.g. `KeyValue` for
 *   `KEY_VALUE`), so the `decoratorcommands` enums are not a semver break;
 *   concerto-codegen 6.3.0 derives neither and names the variants as the
 *   values;
 * - `toRustType`: Integer and Long AST fields are `f64`, since TS reads
 *   them as plain JS numbers (OD-3).
 *
 * Everything else is concerto-codegen's output as is: module layout and
 * imports, field naming and serde renames, `Option`/`Vec` wrapping,
 * `Debug` derives, the `$identifier` and `$timestamp`
 * system fields, and `utils.rs` (whose `serialize_datetime` and
 * `deserialize_datetime` the `$timestamp` fields use).
 */

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');
const { isDeepStrictEqual } = require('util');

const { ModelManager, ModelFile } = require('@accordproject/concerto-core');
const { FileWriter } = require('@accordproject/concerto-util');
const { CodeGen } = require('@accordproject/concerto-codegen');
const CODEGEN_VERSION = require('@accordproject/concerto-codegen/package.json').version;

const CRATE = path.resolve(__dirname, '..');
const VENDOR = path.join(CRATE, 'vendor');
const OUT = path.join(CRATE, 'src', 'generated');
const RECORD = path.join(CRATE, 'codegen.version');
/** The fingerprints `tests/drift.rs` checks, in `src/generated/`. */
const FINGERPRINTS = 'fingerprints.tsv';

/** The models added to the model manager; it adds the other two itself. */
const MODELS = ['concerto.metamodel@1.0.0', 'org.accordproject.decoratorcommands@0.4.0'];
/** The system models the model manager adds, and where it reads them from. */
const SYSTEM_MODELS = {
    'concerto@1.0.0': '@accordproject/concerto-core/dist/rootmodel.json',
    'concerto.decorator@1.0.0': '@accordproject/concerto-core/dist/decoratormodel.json',
};
/** The npm copies of the vendored models, which must match them. */
const NPM_MODELS = {
    'concerto.metamodel@1.0.0': '@accordproject/concerto-metamodel/lib/metamodel.json',
    'org.accordproject.decoratorcommands@0.4.0': '@accordproject/concerto-metamodel/lib/dcsmodel.json',
    ...SYSTEM_MODELS,
};

const METAMODEL = 'concerto.metamodel@1.0.0';

/**
 * concerto-codegen's Rust visitor, with the overrides listed above.
 */
class MetamodelRustVisitor extends CodeGen.RustVisitor {
    /**
     * @param {ModelManager} modelManager - the models being generated
     */
    constructor(modelManager) {
        super();
        this.declarations = modelManager
            .getModelFiles(true)
            .flatMap((modelFile) => modelFile.getAllDeclarations())
            .filter((d) => d.isClassDeclaration?.() && !d.isEnum?.());
        // Types used as the type of a field.
        this.referenced = new Set(
            this.declarations.flatMap((d) =>
                d.getOwnProperties()
                    .filter((p) => !p.isPrimitive())
                    .map((p) => p.getFullyQualifiedTypeName())
            )
        );
    }

    /** The declared super type (not the implicit `concerto@1.0.0.Concept`). */
    superType(cd) {
        return cd.ast.superType ? cd.getSuperTypeDeclaration() : null;
    }

    /** The chain of declared super types, nearest first. */
    ancestors(cd) {
        const chain = [];
        for (let t = this.superType(cd); t; t = this.superType(t)) {
            chain.push(t);
        }
        return chain;
    }

    /** The concrete types that extend `cd`, directly or not, in model order. */
    concreteDescendants(cd) {
        return this.declarations.filter(
            (d) => !d.isAbstract() && this.ancestors(d).includes(cd)
        );
    }

    hasSubtypes(cd) {
        return this.declarations.some((d) => this.superType(d) === cd);
    }

    /** Whether the type is a `$class`-tagged Rust enum. */
    isTaggedEnum(cd) {
        if (this.concreteDescendants(cd).length === 0) {
            return false;
        }
        return cd.isAbstract() || this.referenced.has(cd.getFullyQualifiedName());
    }

    /** Whether the type is only a variant payload, whose `$class` is the tag. */
    isVariant(cd) {
        return this.superType(cd) !== null || this.hasSubtypes(cd);
    }

    /** The properties, the root super type's first. */
    fields(cd) {
        return [...this.ancestors(cd).reverse(), cd].flatMap((t) => t.getOwnProperties());
    }

    /** @override */
    visitClassDeclaration(cd, parameters) {
        const w = parameters.fileWriter;
        const fqn = cd.getFullyQualifiedName();
        const name = cd.getName();
        this.writeDescription(cd, parameters, 0);
        if (this.isTaggedEnum(cd)) {
            w.writeLine(0, `/// \`${fqn}\`, or any concrete type that extends it, selected by \`$class\`.`);
            w.writeLine(0, '#[derive(Debug, Clone, Serialize, Deserialize)]');
            w.writeLine(0, '#[serde(tag = "$class")]');
            w.writeLine(0, '#[allow(clippy::large_enum_variant)]');
            w.writeLine(0, `pub enum ${name} {`);
            if (!cd.isAbstract()) {
                assert.strictEqual(this.fields(cd).length, 0,
                    `${fqn}: a concrete type with fields and sub types cannot be a field type`);
                w.writeLine(1, `#[serde(rename = "${fqn}")]`);
                w.writeLine(1, `${name},`);
            }
            for (const v of this.concreteDescendants(cd)) {
                assert.strictEqual(v.getNamespace(), cd.getNamespace(), `${v.getFullyQualifiedName()}: not in ${fqn}'s namespace`);
                w.writeLine(1, `#[serde(rename = "${v.getFullyQualifiedName()}")]`);
                w.writeLine(1, `${v.getName()}(${v.getName()}),`);
            }
            w.writeLine(0, '}');
            w.writeLine(0, '');
            return null;
        }
        w.writeLine(0, `/// \`${fqn}\``);
        w.writeLine(0, '#[derive(Debug, Clone, Serialize, Deserialize)]');
        w.writeLine(0, `pub struct ${name} {`);
        if (!this.isVariant(cd)) {
            w.writeLine(1, '#[serde(');
            w.writeLine(2, 'rename = "$class",');
            if (cd.getNamespace() === METAMODEL && ['Range', 'Position'].includes(name)) {
                // A declaration's `location` is read, never type-checked, by
                // TS, so v5.0.0 loads a Range or Position without a `$class`
                // (#262); it is written back without one.
                w.writeLine(2, 'default,');
                w.writeLine(2, 'skip_serializing_if = "crate::utils::is_empty_class",');
            }
            w.writeLine(2, 'deserialize_with = "crate::utils::deserialize_class",');
            w.writeLine(1, ')]');
            w.writeLine(1, 'pub _class: crate::ClassName,');
        }
        for (const property of this.fields(cd)) {
            if (!property.isPrimitive()) {
                const target = cd.getModelFile().getModelManager().getType(property.getFullyQualifiedTypeName());
                assert(target.isEnum() || this.isTaggedEnum(target) || !this.isVariant(target),
                    `${fqn}.${property.getName()}: ${target.getFullyQualifiedName()} has no $class of its own to be a field type`);
            }
            w.writeLine(1, '');
            property.accept(this, parameters);
        }
        w.writeLine(0, '}');
        w.writeLine(0, '');
        return null;
    }

    /** @override */
    visitField(field, parameters) {
        const owner = field.getParent?.();
        if (field.getName?.() === 'name' && field.getType() === 'String'
            && owner.getNamespace() === METAMODEL
            && !['ImportType', 'AliasedType'].includes(owner.getName())) {
            assert(!field.isOptional() && !field.isArray(), `${owner.getName()}.name`);
            this.writeDescription(field, parameters, 1);
            parameters.fileWriter.writeLine(1, '#[serde(rename = "name")]');
            parameters.fileWriter.writeLine(1, 'pub name: crate::Name,');
            return null;
        }
        const defaultValue = field.getDefaultValue?.();
        if (defaultValue !== null && defaultValue !== undefined) {
            assert(defaultValue === false && !field.isOptional(),
                `${owner.getName()}.${field.getName()}: default ${defaultValue} is not supported`);
            const inner = parameters.fileWriter;
            const fileWriter = Object.create(inner);
            fileWriter.writeLine = (indent, line) => {
                inner.writeLine(indent, line);
                if (line.startsWith('rename = ')) {
                    inner.writeLine(indent, 'default,');
                }
            };
            return super.visitField(field, { ...parameters, fileWriter });
        }
        return super.visitField(field, parameters);
    }

    /**
     * As the crate's previous generator: a documented enum with `Copy`,
     * `PartialEq` and `Eq`, whose variants are the values in PascalCase,
     * renamed to the value, so the published types keep their API.
     * @override
     */
    visitEnumDeclaration(enumDeclaration, parameters) {
        const w = parameters.fileWriter;
        w.writeLine(0, `/// \`${enumDeclaration.getFullyQualifiedName()}\``);
        w.writeLine(0, '#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]');
        w.writeLine(0, `pub enum ${enumDeclaration.getName()} {`);
        for (const value of enumDeclaration.getOwnProperties()) {
            const name = value.getName();
            w.writeLine(1, `#[serde(rename = "${name}")]`);
            w.writeLine(1, `${pascalCase(name)},`);
        }
        w.writeLine(0, '}');
        w.writeLine(0, '');
        return null;
    }

    /** @override */
    toRustType(type, useUnion) {
        return type === 'Integer' || type === 'Long' ? 'f64' : super.toRustType(type, useUnion);
    }
}

/** `KEY_VALUE` -> `KeyValue`, as the crate's previous generator named enum variants. */
function pascalCase(value) {
    return value
        .split('_')
        .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
        .join('');
}

/**
 * Every declared type's `$class`, sorted by length and then by bytes, for
 * `utils::intern_class`.
 */
function classesTable(modelManager) {
    const classes = modelManager
        .getModelFiles(true)
        .flatMap((modelFile) => modelFile.getAllDeclarations())
        .map((d) => d.getFullyQualifiedName())
        .sort((a, b) => a.length - b.length || (a < b ? -1 : a > b ? 1 : 0));
    return [
        '/// Every declared type\'s `$class`, sorted by length and then by bytes.',
        'pub(crate) const CLASSES: &[&str] = &[',
        ...classes.map((c) => `    ${JSON.stringify(c)},`),
        '];',
        '',
    ].join('\n');
}

function readJson(file) {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
}

/** The checked-in files `src/generated/` is generated from, relative to the crate. */
function inputs() {
    return [
        'codegen/generate.js',
        'codegen/package.json',
        'codegen/package-lock.json',
        ...fs.readdirSync(VENDOR).filter((f) => f.endsWith('.json')).sort().map((f) => `vendor/${f}`),
    ];
}

/**
 * 64-bit FNV-1a of a file's bytes, carriage returns dropped (so a CRLF
 * checkout fingerprints the same), as 16 hex digits. `tests/drift.rs`
 * computes the same; it detects drift, it is not a security check.
 */
function fingerprint(bytes) {
    const MASK = (1n << 64n) - 1n;
    let hash = 0xcbf29ce484222325n;
    for (const byte of bytes) {
        if (byte === 0x0d) {
            continue;
        }
        hash = ((hash ^ BigInt(byte)) * 0x100000001b3n) & MASK;
    }
    return hash.toString(16).padStart(16, '0');
}

/**
 * The fingerprint of every input and generated file, so `tests/drift.rs`
 * can tell, without Node.js, when either changed without a regeneration.
 */
function fingerprints(out) {
    const rows = [
        ...inputs().map((file) => [file, fs.readFileSync(path.join(CRATE, file))]),
        ...fs.readdirSync(out).filter((f) => f !== FINGERPRINTS).sort()
            .map((f) => [`src/generated/${f}`, fs.readFileSync(path.join(out, f))]),
    ];
    return [
        '# @generated by concerto-metamodel/codegen/generate.js. Do not edit.',
        '# FNV-1a 64 (carriage returns dropped) of each input and generated file; checked by tests/drift.rs.',
        ...rows.map(([file, bytes]) => `${fingerprint(bytes)}\t${file}`),
        '',
    ].join('\n');
}

/** Generates the sources into `out`. */
function generate(out) {
    for (const [namespace, module] of Object.entries(NPM_MODELS)) {
        assert(isDeepStrictEqual(readJson(path.join(VENDOR, `${namespace}.json`)), readJson(require.resolve(module))),
            `vendor/${namespace}.json differs from ${module}`);
    }
    const modelManager = new ModelManager({ strict: true });
    modelManager.addModelFiles(MODELS.map((namespace) =>
        new ModelFile(modelManager, readJson(path.join(VENDOR, `${namespace}.json`)), undefined, `${namespace}.json`)));
    assert.deepStrictEqual(
        modelManager.getNamespaces().sort(),
        [...MODELS, ...Object.keys(SYSTEM_MODELS)].sort()
    );

    fs.rmSync(out, { recursive: true, force: true });
    new MetamodelRustVisitor(modelManager).visit(modelManager, { fileWriter: new FileWriter(out) });
    // `lib.rs` declares the modules at the crate root, where the generated
    // `use crate::...` paths point.
    fs.rmSync(path.join(out, 'mod.rs'));
    const header = `// @generated by concerto-metamodel/codegen/generate.js with @accordproject/concerto-codegen@${CODEGEN_VERSION}. Do not edit.\n\n`;
    const files = fs.readdirSync(out).map((f) => path.join(out, f));
    for (const file of files) {
        fs.writeFileSync(file, header + fs.readFileSync(file, 'utf8'));
    }
    const classes = path.join(out, 'classes.rs');
    fs.writeFileSync(classes, header + classesTable(modelManager));
    execFileSync('rustfmt', ['--edition', '2024', ...files, classes], { stdio: 'inherit' });
    fs.writeFileSync(path.join(out, FINGERPRINTS), fingerprints(out));
}

/**
 * `--check`: regenerates into a temporary directory and fails, without
 * touching the crate, when the result differs from `src/generated/` or the
 * recorded version from the pinned one.
 */
function check() {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'concerto-metamodel-'));
    try {
        const out = path.join(tmp, 'generated');
        generate(out);
        const expected = fs.readdirSync(out).sort();
        const actual = fs.readdirSync(OUT).sort();
        const stale = [...new Set([...expected, ...actual])].sort().filter((f) =>
            !expected.includes(f) || !actual.includes(f)
            || !fs.readFileSync(path.join(out, f)).equals(fs.readFileSync(path.join(OUT, f))))
            .map((f) => `src/generated/${f}`);
        if (fs.readFileSync(RECORD, 'utf8') !== `${CODEGEN_VERSION}\n`) {
            stale.push('codegen.version');
        }
        if (stale.length > 0) {
            console.error(`Not what codegen/generate.js generates; run \`npm ci && node generate.js\` in concerto-metamodel/codegen:\n  ${stale.join('\n  ')}`);
            process.exitCode = 1;
        }
    } finally {
        fs.rmSync(tmp, { recursive: true, force: true });
    }
}

if (process.argv.includes('--check')) {
    check();
} else {
    generate(OUT);
    fs.writeFileSync(RECORD, `${CODEGEN_VERSION}\n`);
}
