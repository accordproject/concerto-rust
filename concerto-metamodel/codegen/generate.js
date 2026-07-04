'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const { ModelManager } = require('@accordproject/concerto-core');
const { MetaModelUtil } = require('@accordproject/concerto-metamodel');
const { FileWriter } = require('@accordproject/concerto-util');
const { CodeGen } = require('@accordproject/concerto-codegen');

const outputDir = path.resolve(__dirname, '..', 'src', 'metamodel');

// The model manager preloads the system model; adding the metamodel on top
// yields every namespace the crate exposes.
const modelManager = new ModelManager();
modelManager.addCTOModel(MetaModelUtil.metaModelCto, 'metamodel.cto');

// Generate into a staging directory so a failure cannot leave the crate
// sources half written.
const stagingDir = fs.mkdtempSync(path.join(os.tmpdir(), 'concerto-metamodel-'));
new CodeGen.RustVisitor().visit(modelManager, { fileWriter: new FileWriter(stagingDir) });

// The generator assumes the files sit at the crate root; they live in the
// metamodel module instead, so crate paths become parent module paths.
for (const file of fs.readdirSync(stagingDir)) {
    const staged = path.join(stagingDir, file);
    const source = fs.readFileSync(staged, 'utf8');
    fs.writeFileSync(staged, source.replaceAll('use crate::', 'use super::'));
}

for (const file of fs.readdirSync(outputDir)) {
    if (file.endsWith('.rs')) {
        fs.unlinkSync(path.join(outputDir, file));
    }
}
for (const file of fs.readdirSync(stagingDir)) {
    fs.copyFileSync(path.join(stagingDir, file), path.join(outputDir, file));
}
fs.rmSync(stagingDir, { recursive: true });
