//! `#[auto_configuration]`: the generated `Configuration` and `AutoConfiguration` impls, and the
//! `AutoConfigurations` list. The registration flow is the one boot uses: the configuration's
//! instance (`component::<A>()`) and registrations inside one `group` named by its type, with
//! its class-level conditions.

use std::any::type_name;
use std::collections::HashMap;
use std::sync::Arc;

use oxbloom_boot::{
    AutoConfiguration, AutoConfigurations, Conditions, ConfigRef, Configuration, auto_configuration,
};
use oxbloom_di::{ConditionReport, Container, ContainerBuilder, DiError, Environment, component};

/// A fixed property source.
struct Props(HashMap<&'static str, &'static str>);

impl Props {
    fn of(pairs: &[(&'static str, &'static str)]) -> Self {
        Props(pairs.iter().copied().collect())
    }
}

impl Environment for Props {
    fn property(&self, key: &str) -> Option<String> {
        self.0.get(key).map(|v| (*v).to_owned())
    }
}

/// Appends `A` the way boot does: one group named by the type, with the class conditions,
/// holding the instance and then the registrations.
fn with_auto<A: AutoConfiguration>(builder: ContainerBuilder) -> ContainerBuilder {
    builder.group(type_name::<A>(), A::conditions(), |b| {
        A::register(b.component::<A>())
    })
}

/// The `(condition, matched)` records of `registration`.
fn records(report: &ConditionReport, registration: &str) -> Vec<(String, bool)> {
    report
        .records()
        .iter()
        .filter(|r| r.registration() == registration)
        .map(|r| (r.condition().to_owned(), r.matched()))
        .collect()
}

mod generated {
    use super::*;

    pub struct Pool {
        pub url: &'static str,
    }

    #[component]
    pub struct DataSourceAuto;

    #[auto_configuration(on_property("db.url"))]
    impl DataSourceAuto {
        #[bean]
        fn pool(&self) -> Pool {
            Pool { url: "mem:db" }
        }
    }

    fn build(env: Props) -> Container {
        with_auto::<DataSourceAuto>(ContainerBuilder::new().environment(env))
            .build()
            .unwrap()
    }

    #[test]
    fn macro_generates_configuration_and_auto_configuration() {
        // `Configuration`: the beans register as with `#[configuration]`, no class condition.
        let c = ContainerBuilder::new()
            .component::<DataSourceAuto>()
            .configuration::<DataSourceAuto>()
            .build()
            .unwrap();
        assert_eq!(c.get::<Pool>().unwrap().url, "mem:db");

        // `AutoConfiguration`: the class condition decides the group.
        let c = build(Props::of(&[("db.url", "x")]));
        assert_eq!(c.get::<Pool>().unwrap().url, "mem:db");
        let group = type_name::<DataSourceAuto>();
        assert_eq!(
            records(c.condition_report(), group),
            [("on_property".to_owned(), true)]
        );

        let c = build(Props::of(&[]));
        assert!(c.get_opt::<Pool>().unwrap().is_none());
        assert_eq!(
            records(c.condition_report(), group),
            [("on_property".to_owned(), false)]
        );

        // The methods of the trait are callable on their own.
        assert!(DataSourceAuto::after().is_empty());
        assert!(DataSourceAuto::before().is_empty());
        let _: Conditions = DataSourceAuto::conditions();
        let _ = <DataSourceAuto as Configuration>::register;
    }
}

mod ordering {
    use super::*;

    #[allow(dead_code)]
    pub struct First;
    #[allow(dead_code)]
    pub struct Second;
    #[allow(dead_code)]
    pub struct Third;

    pub mod nested {
        #[allow(dead_code)]
        pub struct Deep;
    }

    #[component]
    struct Ordered;

    // Trailing commas and paths are accepted.
    #[auto_configuration(after(First, Second,), before(nested::Deep))]
    impl Ordered {}

    #[test]
    fn after_and_before_lists() {
        assert_eq!(
            Ordered::after(),
            vec![ConfigRef::of::<First>(), ConfigRef::of::<Second>()]
        );
        assert_eq!(Ordered::before(), vec![ConfigRef::of::<nested::Deep>()]);
        assert_ne!(ConfigRef::of::<First>(), ConfigRef::of::<Second>());
        assert_eq!(ConfigRef::of::<Third>().name(), type_name::<Third>());
    }
}

mod class_conditions {
    use super::*;

    pub struct Marker;
    pub trait Store: Send + Sync {}
    pub struct MemStore;
    impl Store for MemStore {}

    pub struct WithBean;
    pub struct WithoutBean;
    pub struct WithoutStore;
    pub struct WithProperty;
    pub struct WithValue;
    pub struct WithAll;

    #[auto_configuration(on_bean(Marker))]
    impl OnBeanAuto {
        #[bean]
        fn bean(&self) -> WithBean {
            WithBean
        }
    }
    #[component]
    pub struct OnBeanAuto;

    #[auto_configuration(on_missing_bean(Marker))]
    impl OnMissingAuto {
        #[bean]
        fn bean(&self) -> WithoutBean {
            WithoutBean
        }
    }
    #[component]
    pub struct OnMissingAuto;

    #[auto_configuration(on_missing_bean(dyn Store))]
    impl OnMissingStoreAuto {
        #[bean]
        fn bean(&self) -> WithoutStore {
            WithoutStore
        }
    }
    #[component]
    pub struct OnMissingStoreAuto;

    #[auto_configuration(on_property("feature.enabled"))]
    impl OnPropertyAuto {
        #[bean]
        fn bean(&self) -> WithProperty {
            WithProperty
        }
    }
    #[component]
    pub struct OnPropertyAuto;

    #[auto_configuration(on_property("feature.mode", "fast"))]
    impl OnValueAuto {
        #[bean]
        fn bean(&self) -> WithValue {
            WithValue
        }
    }
    #[component]
    pub struct OnValueAuto;

    // Several conditions are ANDed.
    #[auto_configuration(on_bean(Marker), on_property("feature.enabled"))]
    impl AllAuto {
        #[bean]
        fn bean(&self) -> WithAll {
            WithAll
        }
    }
    #[component]
    pub struct AllAuto;

    /// Builds with `user` registered first, then every auto-configuration as a group.
    fn build(user: impl FnOnce(ContainerBuilder) -> ContainerBuilder, env: Props) -> Container {
        let b = user(ContainerBuilder::new().environment(env));
        let b = with_auto::<OnBeanAuto>(b);
        let b = with_auto::<OnMissingAuto>(b);
        let b = with_auto::<OnMissingStoreAuto>(b);
        let b = with_auto::<OnPropertyAuto>(b);
        let b = with_auto::<OnValueAuto>(b);
        with_auto::<AllAuto>(b).build().unwrap()
    }

    fn has<T: Send + Sync + 'static>(c: &Container) -> bool {
        c.get_opt::<T>().unwrap().is_some()
    }

    #[test]
    fn class_conditions_all_forms() {
        // Nothing from the user, no properties.
        let c = build(|b| b, Props::of(&[]));
        assert!(!has::<WithBean>(&c));
        assert!(has::<WithoutBean>(&c));
        assert!(has::<WithoutStore>(&c));
        assert!(!has::<WithProperty>(&c));
        assert!(!has::<WithValue>(&c));
        assert!(!has::<WithAll>(&c));

        // The user's beans and matching properties flip every condition.
        let c = build(
            |b| {
                b.instance(Marker)
                    .instance(MemStore)
                    .provides(|s: Arc<MemStore>| -> Arc<dyn Store> { s })
            },
            Props::of(&[("feature.enabled", "true"), ("feature.mode", "fast")]),
        );
        assert!(has::<WithBean>(&c));
        assert!(!has::<WithoutBean>(&c));
        assert!(!has::<WithoutStore>(&c));
        assert!(has::<WithProperty>(&c));
        assert!(has::<WithValue>(&c));
        assert!(has::<WithAll>(&c));
        let report = c.condition_report();
        assert_eq!(
            records(report, type_name::<AllAuto>()),
            [
                ("on_bean".to_owned(), true),
                ("on_property".to_owned(), true)
            ]
        );
        assert_eq!(
            records(report, type_name::<OnValueAuto>()),
            [("on_property_value".to_owned(), true)]
        );

        // The value is compared exactly; one failing condition of two skips the group.
        let c = build(
            |b| b.instance(Marker),
            Props::of(&[("feature.mode", "faster")]),
        );
        assert!(!has::<WithValue>(&c));
        assert!(!has::<WithAll>(&c));
        assert_eq!(
            records(c.condition_report(), type_name::<AllAuto>()),
            [
                ("on_bean".to_owned(), true),
                ("on_property".to_owned(), false)
            ]
        );
    }
}

mod plain {
    use super::*;

    pub struct Limits {
        pub max: u32,
    }

    #[component]
    pub struct PlainAuto;

    #[auto_configuration]
    impl PlainAuto {
        #[bean]
        fn limits(&self) -> Limits {
            Limits { max: 10 }
        }
    }

    #[component]
    struct EmptyAuto;

    #[auto_configuration()]
    impl EmptyAuto {}

    #[test]
    fn no_options_is_valid() {
        assert!(PlainAuto::after().is_empty());
        assert!(PlainAuto::before().is_empty());
        assert!(EmptyAuto::after().is_empty());
        let c = with_auto::<PlainAuto>(ContainerBuilder::new())
            .build()
            .unwrap();
        assert_eq!(c.get::<Limits>().unwrap().max, 10);
        // No class condition, no group record.
        assert!(records(c.condition_report(), type_name::<PlainAuto>()).is_empty());
        let c = with_auto::<EmptyAuto>(ContainerBuilder::new())
            .build()
            .unwrap();
        assert!(c.condition_report().records().is_empty());
        // The instance is registered even without beans.
        assert!(c.get_opt::<EmptyAuto>().unwrap().is_some());
    }
}

mod bean_options {
    use super::*;

    pub trait Greeter: Send + Sync {
        fn greet(&self) -> &'static str;
    }
    pub struct Cache {
        pub size: usize,
    }
    pub struct English;
    impl Greeter for English {
        fn greet(&self) -> &'static str {
            "hello"
        }
    }
    pub struct DevOnly;
    pub struct Broken;
    pub struct CompiledOut;

    pub struct FailSwitch(pub bool);

    #[component]
    pub struct Defaults {
        switch: Arc<FailSwitch>,
    }

    #[auto_configuration(on_property("defaults.enabled"))]
    impl Defaults {
        #[bean(on_missing_bean)]
        fn cache(&self) -> Cache {
            Cache { size: 16 }
        }

        #[bean(provides(dyn Greeter), primary, lazy)]
        fn english(&self) -> English {
            English
        }

        #[bean(profile = "dev")]
        fn dev_only(&self) -> DevOnly {
            DevOnly
        }

        #[bean(fallible)]
        fn broken(&self) -> Result<Broken, DiError> {
            if self.switch.0 {
                return Err(DiError::creation::<Broken>("refused"));
            }
            Ok(Broken)
        }

        #[cfg(any())]
        #[bean]
        fn compiled_out(&self) -> CompiledOut {
            CompiledOut
        }
    }

    fn build(user: impl FnOnce(ContainerBuilder) -> ContainerBuilder, fail: bool) -> Container {
        let b = user(
            ContainerBuilder::new()
                .environment(Props::of(&[("defaults.enabled", "yes")]))
                .instance(FailSwitch(fail)),
        );
        with_auto::<Defaults>(b).build().unwrap()
    }

    #[test]
    fn bean_options_still_work_inside() {
        let c = build(|b| b, false);
        assert_eq!(c.get::<Cache>().unwrap().size, 16);
        assert_eq!(c.get::<dyn Greeter>().unwrap().greet(), "hello");
        assert!(c.get_opt::<DevOnly>().unwrap().is_none());
        assert!(c.get_opt::<Broken>().unwrap().is_some());
        assert!(c.get_opt::<CompiledOut>().unwrap().is_none());

        // The method-level back-off sees the user's bean; the profile option applies.
        let c = build(
            |b| b.active_profiles(["dev"]).instance(Cache { size: 1024 }),
            false,
        );
        assert_eq!(c.get::<Cache>().unwrap().size, 1024);
        assert!(c.get_opt::<DevOnly>().unwrap().is_some());
        let label = format!("{}::cache", type_name::<Defaults>());
        assert_eq!(
            records(c.condition_report(), &label),
            [("on_missing_bean".to_owned(), false)]
        );

        // `fallible`: the error fails the bean's creation.
        let err = with_auto::<Defaults>(
            ContainerBuilder::new()
                .environment(Props::of(&[("defaults.enabled", "yes")]))
                .instance(FailSwitch(true)),
        )
        .build()
        .err()
        .expect("the fallible bean fails the build");
        assert!(err.to_string().contains("refused"), "{err}");
    }
}

mod siblings {
    use super::*;

    pub struct Foo;
    pub struct Bar;
    #[component]
    pub struct FooAuto;

    // The first bean provides `Foo`; the group is decided once, before it, so `bar` stays.
    #[auto_configuration(on_missing_bean(Foo))]
    impl FooAuto {
        #[bean]
        fn foo(&self) -> Foo {
            Foo
        }

        #[bean]
        fn bar(&self) -> Bar {
            Bar
        }
    }

    #[test]
    fn sibling_beans_see_class_level_group_decided_once() {
        let c = with_auto::<FooAuto>(ContainerBuilder::new())
            .build()
            .unwrap();
        assert!(c.get_opt::<Foo>().unwrap().is_some());
        assert!(c.get_opt::<Bar>().unwrap().is_some());
        assert_eq!(
            records(c.condition_report(), type_name::<FooAuto>()),
            [("on_missing_bean".to_owned(), true)]
        );

        // A user `Foo` registered first skips the whole group.
        let c = with_auto::<FooAuto>(ContainerBuilder::new().instance(Foo))
            .build()
            .unwrap();
        assert!(c.get_opt::<Bar>().unwrap().is_none());
        assert!(c.get_opt::<FooAuto>().unwrap().is_none());
    }
}

mod instance {
    use super::*;

    pub struct Repo {
        pub url: &'static str,
    }
    pub struct Report(pub String);

    /// An auto-configuration with an injected field, used by its bean through `&self`.
    #[component]
    pub struct ReportAuto {
        repo: Arc<Repo>,
    }

    #[auto_configuration(on_bean(Repo))]
    impl ReportAuto {
        #[bean]
        fn report(&self) -> Report {
            Report(format!("report from {}", self.repo.url))
        }
    }

    #[test]
    fn injected_field_is_usable_from_a_bean() {
        let c = with_auto::<ReportAuto>(ContainerBuilder::new().instance(Repo { url: "mem:repo" }))
            .build()
            .unwrap();
        assert_eq!(c.get::<Report>().unwrap().0, "report from mem:repo");
        assert_eq!(c.get::<ReportAuto>().unwrap().repo.url, "mem:repo");
    }

    #[test]
    fn instance_is_skipped_with_its_group() {
        // `on_bean(Repo)` fails: no `Arc<ReportAuto>`, and its missing `Repo` field is never
        // resolved, so the build succeeds.
        let c = with_auto::<ReportAuto>(ContainerBuilder::new())
            .build()
            .unwrap();
        assert!(c.get_opt::<ReportAuto>().unwrap().is_none());
        assert!(c.get_opt::<Report>().unwrap().is_none());
        assert_eq!(
            records(c.condition_report(), type_name::<ReportAuto>()),
            [("on_bean".to_owned(), false)]
        );
    }
}

mod lists {
    use super::*;

    #[component]
    pub struct A;
    #[component]
    pub struct B;
    #[component]
    pub struct C;

    #[auto_configuration]
    impl A {}
    #[auto_configuration(after(A))]
    impl B {}
    #[auto_configuration(before(A))]
    impl C {}

    #[test]
    fn adding_twice_keeps_one_descriptor() {
        let list = AutoConfigurations::new().add::<A>().add::<B>().add::<A>();
        assert_eq!(list.len(), 2);
        assert!(!list.is_empty());
    }

    #[test]
    fn auto_configurations_extend_and_len() {
        assert!(AutoConfigurations::new().is_empty());
        assert_eq!(AutoConfigurations::default().len(), 0);
        let mut list = AutoConfigurations::new().add::<A>().add::<B>();
        list.extend(AutoConfigurations::new().add::<B>().add::<C>());
        assert_eq!(list.len(), 3);
        let copy = list.clone();
        assert_eq!(copy.len(), 3);
        assert!(format!("{copy:?}").contains("AutoConfigurations"));
    }
}

/// The generated code names everything by absolute path.
#[allow(dead_code)]
mod hygiene {
    #![no_implicit_prelude]

    pub struct Value;
    #[::oxbloom_di::component]
    pub struct Hygienic;

    #[::oxbloom_boot::auto_configuration(on_missing_bean(Value), after(Value))]
    impl Hygienic {
        #[bean]
        fn value(&self) -> Value {
            Value
        }
    }
}

#[test]
fn generated_code_is_hygienic() {
    assert_eq!(
        <hygiene::Hygienic as AutoConfiguration>::after(),
        vec![ConfigRef::of::<hygiene::Value>()]
    );
}
