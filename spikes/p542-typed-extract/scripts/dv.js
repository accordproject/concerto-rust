// Does decorateModels throw on the Rust resident path where TS 5.0.0 throws,
// when the source manager has decoratorValidation missingDecorator: 'error'
// and the command set applies a decorator the models do not declare?
const dist = process.argv[2];
const { ModelManager, DecoratorManager } = require(dist);
const ast = {
  $class: 'concerto.metamodel@1.0.0.Model', namespace: 'test@1.0.0', imports: [], decorators: [],
  declarations: [{ $class: 'concerto.metamodel@1.0.0.ConceptDeclaration', name: 'Person', isAbstract: false,
    properties: [{ $class: 'concerto.metamodel@1.0.0.StringProperty', name: 'name', isArray: false, isOptional: false }] }],
};
const dcs = {
  $class: 'org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet', name: 'x', version: '1.0.0',
  commands: [{ $class: 'org.accordproject.decoratorcommands@0.4.0.Command', type: 'UPSERT',
    target: { $class: 'org.accordproject.decoratorcommands@0.4.0.CommandTarget', namespace: 'test@1.0.0', declaration: 'Person' },
    decorator: { $class: 'concerto.metamodel@1.0.0.Decorator', name: 'Undeclared', arguments: [] } }],
};
for (const level of ['error', undefined]) {
  const mm = new ModelManager({ decoratorValidation: { missingDecorator: level, invalidDecorator: level } });
  mm.addCTOModel('namespace test@1.0.0\nconcept Person { o String name }\n', 't.cto');
  try {
    const out = DecoratorManager.decorateModels(mm, dcs);
    console.log(`missingDecorator=${level}: no throw; Person has @Undeclared:`, !!out.getType('test@1.0.0.Person').getDecorator('Undeclared'));
  } catch (e) {
    console.log(`missingDecorator=${level}: threw ${e.constructor.name}: ${e.message.slice(0, 120)}`);
  }
}
