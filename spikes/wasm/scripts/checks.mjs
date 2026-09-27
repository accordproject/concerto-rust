// Functional checks shared by the Node (ESM and CommonJS) and headless-Chromium
// smoke scripts. `engine` is the loaded module: { Engine, setErrorFactory,
// throwSample }. Each check throws on failure; runChecks returns a summary.
import { syntheticModels, invalidModel } from './fixtures.mjs';

function assert(cond, msg) {
  if (!cond) throw new Error(`check failed: ${msg}`);
}

function expectThrow(fn) {
  try {
    fn();
  } catch (e) {
    return e;
  }
  throw new Error('check failed: expected a throw');
}

// A cut-down copy of concerto-core's exception hierarchy, standing in for the
// real classes the P4-02 error mapper will use.
export class BaseException extends Error {
  constructor(message, component) {
    super(message);
    this.component = component;
    this.name = this.constructor.name;
  }
}
export class IllegalModelException extends BaseException {
  constructor(message, modelFile, fileLocation) {
    super(message);
    this.modelFile = modelFile;
    this.fileLocation = fileLocation;
  }
}
export class TypeNotFoundException extends BaseException {
  constructor(typeName, message) {
    super(message ?? `Type "${typeName}" not found`);
    this.typeName = typeName;
  }
}
export class NamespaceNotFoundException extends BaseException {}
export class ValidationException extends BaseException {}

/** The error factory the loader registers: `kind` picks the class. */
export function errorFactory(kind, message, props) {
  let e;
  switch (kind) {
  case 'IllegalModel': e = new IllegalModelException(message, props.fileName, props.location); break;
  case 'TypeNotFound': e = new TypeNotFoundException(props.typeName, message); break;
  case 'NamespaceNotFound': e = new NamespaceNotFoundException(message); break;
  default: e = new ValidationException(message);
  }
  e.kind = kind;
  e.props = props;
  return e;
}

export function runChecks(engine, { metamodelAst }) {
  const { Engine, setErrorFactory, throwSample } = engine;
  const results = {};

  // 1. Synchronous construction: no await anywhere below.
  const e = new Engine();
  const models = syntheticModels({ namespaces: 3, decls: 12, props: 6 });
  e.addModel(JSON.stringify(models[0]), 'n0.cto');
  e.addModelObject(models[1], 'n1.cto');
  e.addModel(JSON.stringify(models[2]));
  e.addModelObject(metamodelAst, 'metamodel.cto');
  e.validateModels();
  results.syncConstructAndValidate = 'ok';

  // 2. The coarse snapshots agree with each other and with the fine getters.
  const fromJson = JSON.parse(e.snapshotJson());
  const fromObj = e.snapshotObject();
  assert(JSON.stringify(fromJson) === JSON.stringify(fromObj), 'snapshotJson == snapshotObject');
  const mm = fromJson.find((f) => f.namespace === 'concerto.metamodel@1.0.0');
  assert(mm && mm.declarations.length === metamodelAst.declarations.length, 'metamodel declaration count');
  const h = e.namespaceHandle('org.spike.n1@1.0.0');
  const snap = fromJson.find((f) => f.namespace === 'org.spike.n1@1.0.0');
  assert(e.hDeclCount(h) === snap.declarations.length, 'hDeclCount');
  snap.declarations.forEach((d, i) => {
    assert(e.declName('org.spike.n1@1.0.0', i) === d.name, 'declName');
    assert(e.hDeclKind(h, i) === d.kind, 'hDeclKind');
    d.properties.forEach((p, j) => {
      assert(e.hPropName(h, i, j) === p.name, 'hPropName');
      assert((e.propType('org.spike.n1@1.0.0', i, j) ?? null) === p.typeName, 'propType');
    });
  });
  results.snapshotsAgree = 'ok';

  // 3. Errors, strategy A: a plain Error built in Rust, fields as own props.
  setErrorFactory(undefined);
  const a = expectThrow(() => throwSample('IllegalModel'));
  assert(a instanceof Error, 'A: instanceof Error');
  assert(a.name === 'IllegalModel', 'A: name');
  assert(a.fileName === 'model.cto' && a.location === 'line 3 column 5', 'A: fileName/location');
  assert(typeof a.stack === 'string', 'A: stack');
  results.errorStrategyA = { name: a.name, message: a.message, fileName: a.fileName, location: a.location };

  // 4. Errors, strategy B: the loader's factory builds the subclass.
  setErrorFactory(errorFactory);
  const b = expectThrow(() => throwSample('IllegalModel'));
  assert(b instanceof IllegalModelException, 'B: instanceof IllegalModelException');
  assert(b instanceof BaseException && b instanceof Error, 'B: instanceof BaseException/Error');
  assert(b.name === 'IllegalModelException', 'B: name');
  assert(b.modelFile === 'model.cto' && b.fileLocation === 'line 3 column 5', 'B: modelFile/fileLocation');
  const t = expectThrow(() => throwSample('TypeNotFound'));
  assert(t instanceof TypeNotFoundException && t.typeName === 'org.acme@1.0.0.Missing', 'B: TypeNotFound');
  results.errorStrategyB = {
    class: b.constructor.name, message: b.message, modelFile: b.modelFile, fileLocation: b.fileLocation,
    stackMentionsWasm: /wasm/.test(b.stack),
  };

  // 5. Real errors from real calls, and the engine survives them.
  const dup = expectThrow(() => e.addModel(JSON.stringify(models[0]), 'again.cto'));
  assert(dup instanceof IllegalModelException && /duplicate namespace/.test(dup.message), 'duplicate namespace');
  assert(dup.modelFile === 'again.cto', 'duplicate namespace fileName');
  const bad = expectThrow(() => e.addModel('{not json', 'broken.cto'));
  assert(bad instanceof IllegalModelException, 'malformed JSON');
  e.addModelObject(invalidModel());
  const v = expectThrow(() => e.validateModels());
  assert(v instanceof ValidationException || v instanceof TypeNotFoundException, `validateModels rejects (${v.constructor.name})`);
  assert(e.namespaceCount() === 5, 'engine still usable after errors');
  results.realErrors = {
    duplicate: `${dup.constructor.name}: ${dup.message}`,
    malformed: `${bad.constructor.name}: ${bad.message}`,
    validate: `${v.constructor.name}: ${v.message}`,
  };
  setErrorFactory(undefined);

  // 6. A Rust panic is not a ConcertoError: it traps. Record what JS sees and
  // whether the instance still works afterwards (reported, not asserted).
  if (engine.panicSample) {
    const p = expectThrow(() => engine.panicSample());
    let afterPanic;
    try {
      const e2 = new Engine();
      e2.addModel(JSON.stringify(models[0]));
      e2.validateModels();
      afterPanic = `usable: ${e2.namespaceCount()} namespace(s) loaded`;
      e2.free();
    } catch (err) {
      afterPanic = `broken: ${err.constructor.name}: ${err.message}`;
    }
    const e3 = new Engine();
    expectThrow(() => e3.panicInMethod());
    let sameObject;
    try {
      sameObject = `usable: namespaceCount() = ${e3.namespaceCount()}`;
    } catch (err) {
      sameObject = `broken: ${err.constructor.name}: ${err.message}`;
    }
    results.panic = {
      thrown: `${p.constructor.name}: ${p.message}`,
      isWebAssemblyRuntimeError: p instanceof WebAssembly.RuntimeError,
      freshObjectAfterPanic: afterPanic,
      objectThatPanickedInAMethod: sameObject,
    };
  }

  // 7. Memory: freeing the engine releases the Rust side.
  e.free();
  const freed = expectThrow(() => e.namespaceCount());
  results.useAfterFree = `${freed.constructor.name}: ${freed.message}`;
  results.finalizationRegistry = typeof FinalizationRegistry === 'function';

  return results;
}
