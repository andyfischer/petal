// Type system for Petal
// This module defines the type system compatible with the goals.

#[derive(Debug, Clone, PartialEq)]
pub enum PetalType {
    Nil,
    Bool,
    Int,
    Float,
    String,
    List(Box<PetalType>),
    Map(Box<PetalType>),
    Function {
        params: Vec<PetalType>,
        return_type: Box<PetalType>,
    },
    Any,
}

impl PetalType {
    pub fn to_string(&self) -> String {
        match self {
            PetalType::Nil => "nil".to_string(),
            PetalType::Bool => "bool".to_string(),
            PetalType::Int => "int".to_string(),
            PetalType::Float => "float".to_string(),
            PetalType::String => "string".to_string(),
            PetalType::List(t) => format!("[{}]", t.to_string()),
            PetalType::Map(t) => format!("map<{}>", t.to_string()),
            PetalType::Function { params, return_type } => {
                let param_str = params
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("fn({}) -> {}", param_str, return_type.to_string())
            }
            PetalType::Any => "any".to_string(),
        }
    }
}
