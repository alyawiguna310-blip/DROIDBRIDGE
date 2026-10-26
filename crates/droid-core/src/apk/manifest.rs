//! Android manifest metadata extracted from binary XML.
//!
//! Only a defined subset of `AndroidManifest.xml` is read: the root `package`,
//! `versionCode`, `versionName`, `uses-sdk`, `uses-permission`,
//! `permission`, the `application` label, and `activity`, `service` and
//! `receiver` names directly under `application`. Everything else is ignored.
//!
//! Values that are resource references (for example `@string/app_name`) are
//! reported as [`ManifestValue::Unresolved`] because `resources.arsc` is not
//! parsed; no value is ever guessed.

use super::axml::{
    ANDROID_NAMESPACE, AttrValue, AxmlError, AxmlLimits, XmlAttribute, XmlEvent, parse_binary_xml,
};
use std::fmt;

/// A manifest value that may be a literal or an unresolved resource reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestValue<T> {
    Literal(T),
    /// A reference to resource `resource_id`, not resolved.
    Unresolved {
        resource_id: u32,
    },
}

/// A `minSdkVersion` or `targetSdkVersion` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkVersion {
    Level(u32),
    /// A preview codename such as `S`.
    Codename(String),
}

/// A declared component (`activity`, `service` or `receiver`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// The `android:name` exactly as declared.
    pub name: String,
    /// The fully qualified class name: `.Foo` and `Foo` are prefixed with the
    /// package, names containing a dot are kept as written.
    pub class_name: String,
}

/// Metadata read from a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInfo {
    pub package: String,
    pub version_code: Option<ManifestValue<u32>>,
    pub version_name: Option<ManifestValue<String>>,
    pub min_sdk: Option<ManifestValue<SdkVersion>>,
    pub target_sdk: Option<ManifestValue<SdkVersion>>,
    /// The `android:label` of the `application` element.
    pub label: Option<ManifestValue<String>>,
    pub uses_permissions: Vec<String>,
    pub declared_permissions: Vec<String>,
    pub activities: Vec<Component>,
    pub services: Vec<Component>,
    pub receivers: Vec<Component>,
}

/// Why a manifest was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The binary XML could not be parsed.
    Xml(AxmlError),
    /// The root element is not `manifest`.
    NotManifestRoot { found: String },
    /// The root element has no non-empty `package` attribute.
    MissingPackageName,
    /// An attribute is missing or has an unsupported type.
    InvalidAttribute {
        element: &'static str,
        attribute: &'static str,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(e) => write!(f, "{e}"),
            Self::NotManifestRoot { found } => {
                write!(f, "root element is {found:?}, expected manifest")
            }
            Self::MissingPackageName => write!(f, "manifest has no package name"),
            Self::InvalidAttribute { element, attribute } => write!(
                f,
                "attribute {attribute} of element {element} is missing or has an unsupported type"
            ),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(e) => Some(e),
            _ => None,
        }
    }
}

impl From<AxmlError> for ManifestError {
    fn from(error: AxmlError) -> Self {
        Self::Xml(error)
    }
}

/// Parse the bytes of a binary `AndroidManifest.xml`.
pub fn parse_manifest(bytes: &[u8], limits: &AxmlLimits) -> Result<ManifestInfo, ManifestError> {
    let events = parse_binary_xml(bytes, limits)?;
    let mut stack: Vec<&str> = Vec::new();
    let mut info: Option<ManifestInfo> = None;

    for event in &events {
        match event {
            XmlEvent::StartElement { name, attributes } => {
                if stack.is_empty() {
                    if name.as_str() != "manifest" {
                        return Err(ManifestError::NotManifestRoot {
                            found: name.clone(),
                        });
                    }
                    info = Some(read_root(attributes)?);
                } else if let Some(info) = info.as_mut() {
                    read_child(info, &stack, name, attributes)?;
                }
                stack.push(name.as_str());
            }
            XmlEvent::EndElement { .. } => {
                stack.pop();
            }
        }
    }

    info.ok_or(ManifestError::Xml(AxmlError::Malformed("no root element")))
}

fn android_attr<'a>(attributes: &'a [XmlAttribute], name: &str) -> Option<&'a AttrValue> {
    attributes
        .iter()
        .find(|a| a.name == name && a.namespace.as_deref() == Some(ANDROID_NAMESPACE))
        .map(|a| &a.value)
}

fn invalid(element: &'static str, attribute: &'static str) -> ManifestError {
    ManifestError::InvalidAttribute { element, attribute }
}

fn string_value(
    value: &AttrValue,
    element: &'static str,
    attribute: &'static str,
) -> Result<ManifestValue<String>, ManifestError> {
    match value {
        AttrValue::String(s) => Ok(ManifestValue::Literal(s.clone())),
        AttrValue::Reference(id) => Ok(ManifestValue::Unresolved { resource_id: *id }),
        _ => Err(invalid(element, attribute)),
    }
}

fn u32_value(
    value: &AttrValue,
    element: &'static str,
    attribute: &'static str,
) -> Result<ManifestValue<u32>, ManifestError> {
    match value {
        AttrValue::Int(n) => Ok(ManifestValue::Literal(*n)),
        AttrValue::Reference(id) => Ok(ManifestValue::Unresolved { resource_id: *id }),
        _ => Err(invalid(element, attribute)),
    }
}

fn sdk_value(
    value: &AttrValue,
    element: &'static str,
    attribute: &'static str,
) -> Result<ManifestValue<SdkVersion>, ManifestError> {
    match value {
        AttrValue::Int(n) => Ok(ManifestValue::Literal(SdkVersion::Level(*n))),
        AttrValue::String(s) => Ok(ManifestValue::Literal(SdkVersion::Codename(s.clone()))),
        AttrValue::Reference(id) => Ok(ManifestValue::Unresolved { resource_id: *id }),
        _ => Err(invalid(element, attribute)),
    }
}

fn required_name(
    attributes: &[XmlAttribute],
    element: &'static str,
) -> Result<String, ManifestError> {
    match android_attr(attributes, "name") {
        Some(AttrValue::String(s)) => Ok(s.clone()),
        _ => Err(invalid(element, "name")),
    }
}

fn read_root(attributes: &[XmlAttribute]) -> Result<ManifestInfo, ManifestError> {
    let package = match attributes
        .iter()
        .find(|a| a.namespace.is_none() && a.name == "package")
    {
        Some(XmlAttribute {
            value: AttrValue::String(s),
            ..
        }) if !s.is_empty() => s.clone(),
        Some(XmlAttribute {
            value: AttrValue::String(_),
            ..
        })
        | None => return Err(ManifestError::MissingPackageName),
        Some(_) => return Err(invalid("manifest", "package")),
    };

    Ok(ManifestInfo {
        package,
        version_code: android_attr(attributes, "versionCode")
            .map(|v| u32_value(v, "manifest", "versionCode"))
            .transpose()?,
        version_name: android_attr(attributes, "versionName")
            .map(|v| string_value(v, "manifest", "versionName"))
            .transpose()?,
        min_sdk: None,
        target_sdk: None,
        label: None,
        uses_permissions: Vec::new(),
        declared_permissions: Vec::new(),
        activities: Vec::new(),
        services: Vec::new(),
        receivers: Vec::new(),
    })
}

fn read_child(
    info: &mut ManifestInfo,
    stack: &[&str],
    name: &str,
    attributes: &[XmlAttribute],
) -> Result<(), ManifestError> {
    match (stack.len(), stack.last().copied()) {
        (1, Some("manifest")) => match name {
            "uses-sdk" => {
                if let Some(v) = android_attr(attributes, "minSdkVersion") {
                    info.min_sdk = Some(sdk_value(v, "uses-sdk", "minSdkVersion")?);
                }
                if let Some(v) = android_attr(attributes, "targetSdkVersion") {
                    info.target_sdk = Some(sdk_value(v, "uses-sdk", "targetSdkVersion")?);
                }
            }
            "uses-permission" => {
                info.uses_permissions
                    .push(required_name(attributes, "uses-permission")?);
            }
            "uses-permission-sdk-23" => {
                info.uses_permissions
                    .push(required_name(attributes, "uses-permission-sdk-23")?);
            }
            "permission" => {
                info.declared_permissions
                    .push(required_name(attributes, "permission")?);
            }
            "application" => {
                if let Some(v) = android_attr(attributes, "label") {
                    info.label = Some(string_value(v, "application", "label")?);
                }
            }
            _ => {}
        },
        (2, Some("application")) => match name {
            "activity" => {
                let component = component(&info.package, attributes, "activity")?;
                info.activities.push(component);
            }
            "service" => {
                let component = component(&info.package, attributes, "service")?;
                info.services.push(component);
            }
            "receiver" => {
                let component = component(&info.package, attributes, "receiver")?;
                info.receivers.push(component);
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

fn component(
    package: &str,
    attributes: &[XmlAttribute],
    element: &'static str,
) -> Result<Component, ManifestError> {
    let name = required_name(attributes, element)?;
    let class_name = resolve_class_name(package, &name);
    Ok(Component { name, class_name })
}

/// Android's rule for component names: a leading dot or a name without any
/// dot is relative to the package.
fn resolve_class_name(package: &str, name: &str) -> String {
    if name.starts_with('.') {
        format!("{package}{name}")
    } else if name.contains('.') {
        name.to_owned()
    } else {
        format!("{package}.{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apk::testutil::{
        TestValue, android, build_axml, end, minimal_manifest_axml, plain, sample_manifest_axml,
        start,
    };

    fn parse(bytes: &[u8]) -> Result<ManifestInfo, ManifestError> {
        parse_manifest(bytes, &AxmlLimits::default())
    }

    fn s(text: &str) -> TestValue {
        TestValue::Str(text.to_owned())
    }

    #[test]
    fn extracts_all_supported_fields() {
        for utf8 in [false, true] {
            let info = parse(&sample_manifest_axml(utf8)).unwrap();
            assert_eq!(info.package, "com.example.app");
            assert_eq!(info.version_code, Some(ManifestValue::Literal(7)));
            assert_eq!(
                info.version_name,
                Some(ManifestValue::Literal("1.2.3".to_owned()))
            );
            assert_eq!(
                info.min_sdk,
                Some(ManifestValue::Literal(SdkVersion::Level(21)))
            );
            assert_eq!(
                info.target_sdk,
                Some(ManifestValue::Literal(SdkVersion::Level(34)))
            );
            assert_eq!(
                info.label,
                Some(ManifestValue::Literal("Example".to_owned()))
            );
            assert_eq!(info.uses_permissions, vec!["android.permission.INTERNET"]);
            assert_eq!(info.declared_permissions, vec!["com.example.app.PRIVATE"]);
            assert_eq!(
                info.activities,
                vec![Component {
                    name: ".MainActivity".to_owned(),
                    class_name: "com.example.app.MainActivity".to_owned(),
                }]
            );
            assert_eq!(
                info.services,
                vec![Component {
                    name: "com.example.app.SyncService".to_owned(),
                    class_name: "com.example.app.SyncService".to_owned(),
                }]
            );
            assert_eq!(
                info.receivers,
                vec![Component {
                    name: "BootReceiver".to_owned(),
                    class_name: "com.example.app.BootReceiver".to_owned(),
                }]
            );
        }
    }

    #[test]
    fn missing_optional_metadata_is_none_or_empty() {
        let info = parse(&minimal_manifest_axml("com.example.min")).unwrap();
        assert_eq!(info.package, "com.example.min");
        assert_eq!(info.version_code, None);
        assert_eq!(info.version_name, None);
        assert_eq!(info.min_sdk, None);
        assert_eq!(info.target_sdk, None);
        assert_eq!(info.label, None);
        assert!(info.uses_permissions.is_empty());
        assert!(info.declared_permissions.is_empty());
        assert!(info.activities.is_empty());
        assert!(info.services.is_empty());
        assert!(info.receivers.is_empty());
    }

    #[test]
    fn resource_references_stay_unresolved() {
        let nodes = vec![
            start(
                "manifest",
                vec![
                    plain("package", s("com.example.ref")),
                    android("versionCode", TestValue::Reference(0x7f02_0001)),
                    android("versionName", TestValue::Reference(0x7f03_0002)),
                ],
            ),
            start(
                "application",
                vec![android("label", TestValue::Reference(0x7f03_0001))],
            ),
            end(),
            end(),
        ];
        let info = parse(&build_axml(&nodes, false)).unwrap();
        assert_eq!(
            info.label,
            Some(ManifestValue::Unresolved {
                resource_id: 0x7f03_0001
            })
        );
        assert_eq!(
            info.version_name,
            Some(ManifestValue::Unresolved {
                resource_id: 0x7f03_0002
            })
        );
        assert_eq!(
            info.version_code,
            Some(ManifestValue::Unresolved {
                resource_id: 0x7f02_0001
            })
        );
    }

    #[test]
    fn sdk_codenames_are_kept_as_codenames() {
        let nodes = vec![
            start("manifest", vec![plain("package", s("com.example.sdk"))]),
            start("uses-sdk", vec![android("minSdkVersion", s("S"))]),
            end(),
            end(),
        ];
        let info = parse(&build_axml(&nodes, false)).unwrap();
        assert_eq!(
            info.min_sdk,
            Some(ManifestValue::Literal(SdkVersion::Codename("S".to_owned())))
        );
        assert_eq!(info.target_sdk, None);
    }

    #[test]
    fn rejects_non_manifest_root() {
        let nodes = vec![start("application", vec![]), end()];
        assert_eq!(
            parse(&build_axml(&nodes, false)),
            Err(ManifestError::NotManifestRoot {
                found: "application".to_owned()
            })
        );
    }

    #[test]
    fn rejects_missing_or_empty_package() {
        let missing = vec![start("manifest", vec![]), end()];
        assert_eq!(
            parse(&build_axml(&missing, false)),
            Err(ManifestError::MissingPackageName)
        );
        let empty = vec![start("manifest", vec![plain("package", s(""))]), end()];
        assert_eq!(
            parse(&build_axml(&empty, false)),
            Err(ManifestError::MissingPackageName)
        );
        let wrong_type = vec![
            start("manifest", vec![plain("package", TestValue::Int(1))]),
            end(),
        ];
        assert_eq!(
            parse(&build_axml(&wrong_type, false)),
            Err(ManifestError::InvalidAttribute {
                element: "manifest",
                attribute: "package"
            })
        );
    }

    #[test]
    fn rejects_attributes_of_the_wrong_type() {
        let nodes = vec![
            start(
                "manifest",
                vec![
                    plain("package", s("com.example.bad")),
                    android("versionCode", s("seven")),
                ],
            ),
            end(),
        ];
        assert_eq!(
            parse(&build_axml(&nodes, false)),
            Err(ManifestError::InvalidAttribute {
                element: "manifest",
                attribute: "versionCode"
            })
        );
    }

    #[test]
    fn components_need_a_name() {
        let nodes = vec![
            start("manifest", vec![plain("package", s("com.example.c"))]),
            start("application", vec![]),
            start("activity", vec![]),
            end(),
            end(),
            end(),
        ];
        assert_eq!(
            parse(&build_axml(&nodes, false)),
            Err(ManifestError::InvalidAttribute {
                element: "activity",
                attribute: "name"
            })
        );
    }

    #[test]
    fn components_outside_application_are_ignored() {
        let nodes = vec![
            start("manifest", vec![plain("package", s("com.example.o"))]),
            start("activity", vec![android("name", s(".Stray"))]),
            end(),
            start("application", vec![]),
            start("meta-data", vec![]),
            start("activity", vec![android("name", s(".Nested"))]),
            end(),
            end(),
            end(),
            end(),
        ];
        let info = parse(&build_axml(&nodes, false)).unwrap();
        assert!(info.activities.is_empty());
    }

    #[test]
    fn xml_errors_are_propagated() {
        assert_eq!(
            parse(b"<manifest package=x/>"),
            Err(ManifestError::Xml(AxmlError::NotBinaryXml))
        );
        let bytes = sample_manifest_axml(false);
        assert!(matches!(
            parse(&bytes[..bytes.len() / 2]),
            Err(ManifestError::Xml(AxmlError::Truncated { .. }))
        ));
    }

    #[test]
    fn class_names_resolve_like_android() {
        assert_eq!(resolve_class_name("com.a", ".Main"), "com.a.Main");
        assert_eq!(resolve_class_name("com.a", "Main"), "com.a.Main");
        assert_eq!(resolve_class_name("com.a", "org.b.Main"), "org.b.Main");
    }
}
