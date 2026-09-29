// Same decorator validation, without DecoratorManager: a model that uses an
// undeclared decorator, added to a manager with missingDecorator: 'error'.
const { ModelManager } = require(process.argv[2]);
const mm = new ModelManager({ decoratorValidation: { missingDecorator: 'error', invalidDecorator: 'error' } });
try { mm.addCTOModel('namespace test@1.0.0\n@Undeclared\nconcept Person { o String name }\n', 't.cto'); console.log('addCTOModel: no throw'); }
catch (e) { console.log('addCTOModel threw', e.constructor.name, e.message.slice(0, 100)); }
const mm2 = new ModelManager({ decoratorValidation: { missingDecorator: 'error', invalidDecorator: 'error' } });
const src = new ModelManager(); src.addCTOModel('namespace test@1.0.0\n@Undeclared\nconcept Person { o String name }\n', 't.cto');
try { mm2.fromAst(src.getAst()); console.log('fromAst: no throw'); }
catch (e) { console.log('fromAst threw', e.constructor.name, e.message.slice(0, 100)); }
