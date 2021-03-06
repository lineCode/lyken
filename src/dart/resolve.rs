use dart::ast::{ClassMember, Expr, FnName, ForLoop, Function, ImportFilter, Item, Module,
                Qualified, Statement, TryPart, TypeParameter, VarDef};
use dart::visit::{Visit, VisitNode, Visitor};
use dart::sdk;
use dsl;
use node::Node;
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;
use syntax::symbol::Symbol;


#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Res {
    // Types.
    Void,
    Dynamic,
    Module(Node<Module>),
    Class(Node<Item>),
    Enum(Node<Item>),
    TypeAlias(Node<Item>),
    TypeParameter(Node<TypeParameter>),

    // Values.
    Null,
    False,
    True,
    This,
    Super,
    RuntimeType,
    Var(Node<VarDef>),
    Function(Node<Function>),
    Getter(Node<Function>),
    Setter(Node<Function>),
    GetterSetter(Node<Function>, Node<Function>),
    Constructor(Node<ClassMember>),

    Dsl(dsl::resolve::Res),

    Error,
}

impl Res {
    pub fn lookup_member(&self, name: Symbol) -> Res {
        match *self {
            Res::Class(ref item) => item.exports().lookup(name),
            Res::Module(ref module) => module.exports().lookup(name),
            _ => Res::Error,
        }
    }
}

#[derive(Clone)]
pub struct ScopeChain {
    parent: Option<Rc<ScopeChain>>,
    map: HashMap<Symbol, Res>,
}

impl ScopeChain {
    pub fn new(parent: Option<Rc<ScopeChain>>) -> Rc<ScopeChain> {
        Rc::new(ScopeChain {
            parent: parent,
            map: HashMap::new(),
        })
    }
    pub fn lookup(&self, name: Symbol) -> Res {
        if let Some(res) = self.map.get(&name) {
            res.clone()
        } else if let Some(ref parent) = self.parent {
            parent.lookup(name)
        } else {
            Res::Error
        }
    }
    pub fn extend(&mut self, scope: &ScopeChain) {
        if let Some(ref parent) = scope.parent {
            self.extend(parent);
        }
        self.map
            .extend(scope.map.iter().map(|(&k, v)| (k, v.clone())));
    }
}

node_field!(cached_exports: Rc<ScopeChain>);

impl<T: VisitNode> Node<T> {
    pub fn exports(&self) -> Rc<ScopeChain> {
        if let Some(exports) = self.cached_exports().get() {
            return exports;
        }
        let mut collector = Collector {
            scope: ScopeChain::new(None),
            exports_only: true,
            has_error: false,
        };
        if let Some(item) = (self.clone() as Node<Any>).downcast() {
            if let Item::Class { ref superclass, .. } = *item {
                if let Some(ref superclass) = *superclass {
                    if superclass.res().get().is_none() {
                        resolve(item.root_module().unwrap(), false);
                    }
                    if let Some(Res::Class(superclass)) = superclass.res().get() {
                        collector.scope_mut().parent = Some(superclass.exports());
                    } else {
                        println!("unknown superclass {:?}", superclass);
                    }
                }
            }
            match *item {
                Item::Class {
                    ref mixins,
                    ref interfaces,
                    ..
                } |
                Item::MixinClass {
                    ref mixins,
                    ref interfaces,
                    ..
                } => for class in mixins.iter().chain(interfaces) {
                    if class.res().get().is_none() {
                        resolve(item.root_module().unwrap(), false);
                    }
                    if let Some(Res::Class(class)) = class.res().get() {
                        collector.scope_mut().extend(&class.exports());
                    } else {
                        println!("unknown class {:?}", class);
                    }
                },
                _ => {}
            }
        }
        self.super_visit(&mut collector);
        self.cached_exports().set(collector.scope.clone());
        collector.scope
    }
}

node_field!(res: Res);

pub fn resolve(module: Node<Module>, fully_resolve: bool) {
    let collector = &mut Collector::new();

    if module != Module::load(&sdk::resolve_import("dart:core")) {
        collector.import(None, "dart:core", &[], None);
    }

    module.super_visit(collector);
    if collector.has_error {
        return;
    }

    module.super_visit(&mut TopLevelResolver { collector });
    if fully_resolve {
        module.super_visit(&mut Resolver { collector });
    }
}

pub struct Collector {
    scope: Rc<ScopeChain>,
    exports_only: bool,
    pub has_error: bool,
}

impl Collector {
    pub fn new() -> Collector {
        let mut collector = Collector {
            scope: ScopeChain::new(None),
            exports_only: false,
            has_error: false,
        };
        collector.record("void", Res::Void);
        collector.record("dynamic", Res::Dynamic);
        collector.record("null", Res::Null);
        collector.record("false", Res::False);
        collector.record("true", Res::True);
        collector
    }

    fn scope_mut(&mut self) -> &mut ScopeChain {
        Rc::make_mut(&mut self.scope)
    }

    pub fn record<S: Into<Symbol>>(&mut self, name: S, res: Res) {
        self.scope_mut().map.insert(name.into(), res);
    }

    pub fn import(
        &mut self,
        root_module: Option<Node<Module>>,
        uri: &str,
        filters: &[ImportFilter],
        alias: Option<Symbol>,
    ) {
        let mut path = sdk::resolve_import(uri);
        if path.is_relative() {
            path = root_module.unwrap().path.parent().unwrap().join(&path);
        }
        let module = Module::load(&path);
        if module.has_error {
            self.has_error = true;
        }
        if let Some(alias) = alias {
            self.record(alias, Res::Module(module.clone()));
        }
        let mut scope = module.exports();
        for filter in filters {
            if filter.hide {
                for &name in &filter.names {
                    Rc::make_mut(&mut scope).map.remove(&name);
                }
            } else {
                for &name in &filter.names {
                    self.record(name, scope.lookup(name));
                }
                return;
            }
        }
        self.scope_mut().extend(&scope);
    }
}

impl Visitor for Collector {
    fn dart_item(&mut self, item: Node<Item>) {
        match *item {
            Item::Class { name, .. } | Item::MixinClass { name, .. } => {
                self.record(name, Res::Class(item.clone()));
            }
            Item::Enum { name, .. } => {
                self.record(name, Res::Enum(item.clone()));
            }
            Item::TypeAlias { name, .. } => {
                self.record(name, Res::TypeAlias(item.clone()));
            }
            Item::Function { ref function, .. } => {
                function.visit(self);
            }
            Item::Vars(_, _, ref vars) => for var in vars {
                var.visit(self);
            },
            Item::Part { ref module, .. } => {
                module.super_visit(self);
            }
            Item::Export(_, ref uri, ref filters) => if self.exports_only {
                self.import(item.root_module(), &uri.get_simple_string(), filters, None);
            },
            Item::Import(_, ref import) => if !import.deferred && !self.exports_only {
                self.import(
                    item.root_module(),
                    &import.uri.get_simple_string(),
                    &import.filters,
                    import.alias,
                );
            },
            _ => {}
        }
    }
    fn dart_class_member(&mut self, class_member: Node<ClassMember>) {
        match *class_member {
            ClassMember::Redirect {
                name: Some(name), ..
            } |
            ClassMember::Constructor {
                name: Some(name), ..
            } => {
                self.record(name, Res::Constructor(class_member.clone()));
            }
            ClassMember::Method(_, _, ref function) => {
                function.visit(self);
            }
            ClassMember::Fields {
                ref initializers, ..
            } => for field in initializers {
                field.visit(self);
            },
            _ => {}
        }
    }
    fn dart_function(&mut self, function: Node<Function>) {
        match function.name {
            FnName::Regular(name) => {
                self.record(name, Res::Function(function.clone()));
            }
            FnName::Getter(name) => {
                self.record(name, Res::Getter(function.clone()));
            }
            FnName::Setter(name) => {
                self.record(name, Res::Setter(function.clone()));
            }
            _ => {}
        }
    }
    fn dart_var_def(&mut self, var: Node<VarDef>) {
        self.record(var.name, Res::Var(var.clone()));
    }
}

pub struct TopLevelResolver<'a> {
    pub collector: &'a mut Collector,
}

impl<'a> Visitor for TopLevelResolver<'a> {
    fn dart_qualified(&mut self, qualified: Node<Qualified>) {
        for ty in &qualified.params {
            ty.visit(self);
        }

        if qualified.res().get().is_some() {
            return;
        }

        let res = if let Some(ref prefix) = qualified.prefix {
            prefix.visit(self);
            if let Some(prefix_res @ Res::Module(_)) = prefix.res().get() {
                prefix_res.lookup_member(qualified.name)
            } else {
                Res::Error
            }
        } else {
            self.collector.scope.lookup(qualified.name)
        };

        if let Res::Error = res {
            return;
        }
        qualified.res().set(res);
    }
}

pub struct Resolver<'a> {
    pub collector: &'a mut Collector,
}

impl<'a> Resolver<'a> {
    pub fn in_lexical_scope<F: FnOnce(&mut Self) -> R, R>(&mut self, f: F) -> R {
        let parent = self.collector.scope.clone();
        self.collector.scope = ScopeChain::new(Some(parent.clone()));
        let result = f(self);
        self.collector.scope = parent;
        result
    }
}

impl<'a> Visitor for Resolver<'a> {
    fn dart_item(&mut self, item: Node<Item>) {
        self.in_lexical_scope(|this| {
            if let Item::Class {
                ref superclass,
                ref mixins,
                ref interfaces,
                ..
            } = *item
            {
                this.collector.record("this", Res::This);
                this.collector.record("super", Res::Super);
                this.collector.record("runtimeType", Res::RuntimeType);

                for class in superclass.iter().chain(mixins).chain(interfaces) {
                    class.visit(&mut TopLevelResolver {
                        collector: this.collector,
                    });
                    if let Some(Res::Class(class)) = class.res().get() {
                        this.collector.scope_mut().extend(&class.exports());
                    } else {
                        println!("unknown class {:?}", class);
                    }
                }

                item.super_visit(this.collector);
            }
            item.super_visit(this);
        });
    }
    fn dart_function(&mut self, function: Node<Function>) {
        function.visit(self.collector);
        self.in_lexical_scope(|this| function.super_visit(this));
    }
    fn dart_try_part(&mut self, try_part: &TryPart) {
        self.in_lexical_scope(|this| try_part.super_visit(this));
    }
    fn dart_statement(&mut self, statement: Node<Statement>) {
        if let Statement::For(_, ForLoop::InVar(..), _) = *statement {
            self.in_lexical_scope(|this| statement.super_visit(this));
        } else {
            statement.super_visit(self);
        }
    }
    fn dart_block(&mut self, statements: &[Node<Statement>]) {
        self.in_lexical_scope(|this| statements.super_visit(this));
    }
    fn dart_var_def(&mut self, var: Node<VarDef>) {
        var.visit(self.collector);
        var.super_visit(self);
    }
    fn dart_expr(&mut self, expr: Node<Expr>) {
        if expr.res().get().is_some() {
            return;
        }

        if let Expr::Identifier(name) = *expr {
            let res = self.collector.scope.lookup(name);
            if let Res::Error = res {
                println!("unknown value {}", name);
            }
            expr.res().set(res);
        }
        expr.super_visit(self)
    }
    fn dart_qualified(&mut self, qualified: Node<Qualified>) {
        for ty in &qualified.params {
            ty.visit(self);
        }

        if qualified.res().get().is_some() {
            return;
        }

        let res = if let Some(ref prefix) = qualified.prefix {
            prefix.visit(self);
            prefix.res().get().unwrap().lookup_member(qualified.name)
        } else {
            self.collector.scope.lookup(qualified.name)
        };

        if let Res::Error = res {
            println!("unknown path {:?}", qualified);
        }
        qualified.res().set(res);
    }
    fn dart_generics(&mut self, generics: &[Node<TypeParameter>]) {
        for generic in generics {
            self.collector
                .record(generic.name, Res::TypeParameter(generic.clone()));
        }
        generics.super_visit(self)
    }
}
