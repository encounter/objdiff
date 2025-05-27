use std::fmt;

// File generated wholesale by Claude Sonnet 4.0, prompt:
//
// I have a C/C++ function signature which does not include the return type
// because I got it from de-mangling symbol names. The data does include
// namespaced types and template types.
// E.g.: "zFrag_DefaultInit(zFrag, zFragAsset)". No return type and no
// semi-colon at the end.
//
// I'm writing rust code which needs to know the argument types. I want to know
// the argument types so that I can determine what registers are being used to
// pass which arguments.
//
// Parse the signature and return structured data. Template and function pointer
// types must be handled but you can treat them as opaque types in the output, I
// don't need their full details.

#[derive(Debug, Clone, PartialEq)]
pub struct ArgumentType {
    pub base_type: String,
    pub is_pointer: bool,
    pub pointer_depth: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSignature {
    pub name: String,
    pub arguments: Vec<ArgumentType>,
}

impl fmt::Display for ArgumentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut result = String::new();
        
        result.push_str(&self.base_type);
        
        for _ in 0..self.pointer_depth {
            result.push('*');
        }
        
        write!(f, "{}", result)
    }
}

pub fn parse_cpp_signature(signature: &str) -> Result<FunctionSignature, String> {
    let signature = signature.trim();
    
    // Find the opening parenthesis
    let paren_start = signature.find('(')
        .ok_or("No opening parenthesis found")?;
    
    // Extract function name
    let function_name = signature[..paren_start].trim().to_string();
    
    // Find matching closing parenthesis
    let paren_end = find_matching_paren(signature, paren_start)?;
    
    // Extract arguments string
    let args_str = &signature[paren_start + 1..paren_end].trim();
    
    // Parse arguments
    let arguments = if args_str.is_empty() {
        Vec::new()
    } else {
        parse_arguments(args_str)?
    };
    
    Ok(FunctionSignature {
        name: function_name,
        arguments,
    })
}

fn find_matching_paren(s: &str, start: usize) -> Result<usize, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut depth = 0;
    let mut in_template = 0;
    
    for i in start..chars.len() {
        match chars[i] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            '<' => {
                // Only count as template if not inside another template with comparison
                if i > 0 && (chars[i-1].is_alphanumeric() || chars[i-1] == '_' || chars[i-1] == ':') {
                    in_template += 1;
                }
            }
            '>' => {
                if in_template > 0 {
                    in_template -= 1;
                }
            }
            _ => {}
        }
    }
    
    Err("No matching closing parenthesis found".to_string())
}

fn parse_arguments(args_str: &str) -> Result<Vec<ArgumentType>, String> {
    let mut arguments = Vec::new();
    let mut current_arg = String::new();
    let mut paren_depth = 0;
    let mut template_depth = 0;
    let chars: Vec<char> = args_str.chars().collect();
    let mut i = 0;
    
    while i < chars.len() {
        let ch = chars[i];
        
        match ch {
            '(' => {
                paren_depth += 1;
                current_arg.push(ch);
            }
            ')' => {
                paren_depth -= 1;
                current_arg.push(ch);
            }
            '<' => {
                // Check if this is a template opening
                if i > 0 && (chars[i-1].is_alphanumeric() || chars[i-1] == '_' || chars[i-1] == ':') {
                    template_depth += 1;
                }
                current_arg.push(ch);
            }
            '>' => {
                current_arg.push(ch);
                if template_depth > 0 {
                    template_depth -= 1;
                }
            }
            ',' => {
                if paren_depth == 0 && template_depth == 0 {
                    // End of current argument
                    let arg_type = parse_single_argument(current_arg.trim())?;
                    arguments.push(arg_type);
                    current_arg.clear();
                } else {
                    current_arg.push(ch);
                }
            }
            _ => {
                current_arg.push(ch);
            }
        }
        
        i += 1;
    }
    
    // Handle the last argument
    if !current_arg.trim().is_empty() {
        let arg_type = parse_single_argument(current_arg.trim())?;
        arguments.push(arg_type);
    }
    
    Ok(arguments)
}

fn parse_single_argument(arg_str: &str) -> Result<ArgumentType, String> {
    let arg_str = arg_str.trim();
    
    if arg_str.is_empty() {
        return Err("Empty argument".to_string());
    }
    
    let mut pointer_depth = 0;
    
    // Split into tokens while preserving template brackets and function pointers
    let tokens = tokenize_type(arg_str);
    let mut i = 0;
    
    // Check for const at the beginning
    if i < tokens.len() && tokens[i] == "const" {
        i += 1;
    }
    
    // Build the base type (everything until we hit pointers/references)
    let mut type_tokens = Vec::new();
    while i < tokens.len() {
        let token = &tokens[i];
        if token == "*" {
            break;
        } else if token == "&" {
            break;
        } else {
            type_tokens.push(token.clone());
            i += 1;
        }
    }
    
    let mut base_type = type_tokens.join(" ");
    
    // Count pointers and check for reference
    while i < tokens.len() {
        match tokens[i].as_str() {
            "*" | "&" => pointer_depth += 1,
            "const" => {
                // const after * (like int* const) - we can ignore this for register purposes
            }
            _ => {
                // Unexpected token after type
                return Err(format!("Unexpected token '{}' in type", tokens[i]));
            }
        }
        i += 1;
    }
    
    // Handle special case where the type might be entirely pointers (like void*)
    if base_type.is_empty() && pointer_depth > 0 {
        base_type = "void".to_string();
    }
    
    // Quick and dirty way to find out if it's a function pointer since the
    // other pointer logic relies on the */& being at the end.
    let is_fn_pointer = base_type.contains("(*)");

    Ok(ArgumentType {
        base_type,
        is_pointer: pointer_depth > 0 || is_fn_pointer,
        pointer_depth,
    })
}

fn tokenize_type(type_str: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current_token = String::new();
    let chars: Vec<char> = type_str.chars().collect();
    let mut i = 0;
    
    while i < chars.len() {
        let ch = chars[i];
        
        match ch {
            ' ' | '\t' | '\n' => {
                if !current_token.is_empty() {
                    tokens.push(current_token.clone());
                    current_token.clear();
                }
                // Skip whitespace
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                i -= 1; // Adjust for the increment at the end of the loop
            }
            '*' | '&' => {
                if !current_token.is_empty() {
                    tokens.push(current_token.clone());
                    current_token.clear();
                }
                tokens.push(ch.to_string());
            }
            '<' => {
                // Handle template - consume everything until matching >
                current_token.push(ch);
                i += 1;
                let mut template_depth = 1;
                
                while i < chars.len() && template_depth > 0 {
                    let template_ch = chars[i];
                    current_token.push(template_ch);
                    
                    match template_ch {
                        '<' => template_depth += 1,
                        '>' => template_depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                i -= 1; // Adjust for the increment at the end of the loop
            }
            '(' => {
                // Handle function pointer - consume everything until matching )
                current_token.push(ch);
                i += 1;
                let mut paren_depth = 1;
                
                while i < chars.len() && paren_depth > 0 {
                    let paren_ch = chars[i];
                    current_token.push(paren_ch);
                    
                    match paren_ch {
                        '(' => paren_depth += 1,
                        ')' => paren_depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                i -= 1; // Adjust for the increment at the end of the loop
            }
            _ => {
                current_token.push(ch);
            }
        }
        
        i += 1;
    }
    
    if !current_token.is_empty() {
        tokens.push(current_token);
    }
    
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_function() {
        let sig = parse_cpp_signature("zFrag_DefaultInit(zFrag*, zFragAsset*)").unwrap();
        assert_eq!(sig.name, "zFrag_DefaultInit");
        assert_eq!(sig.arguments.len(), 2);
        
        assert_eq!(sig.arguments[0].base_type, "zFrag");
        assert_eq!(sig.arguments[0].pointer_depth, 1);
        assert!(sig.arguments[0].is_pointer);
        
        assert_eq!(sig.arguments[1].base_type, "zFragAsset");
        assert_eq!(sig.arguments[1].pointer_depth, 1);
        assert!(sig.arguments[1].is_pointer);
    }

    #[test]
    fn test_complex_example_function() {
        let sig = parse_cpp_signature("zShrapnel_DestructObjInit(zShrapnelAsset*, xModelInstance*, xVec3*, void (*)(zFrag*, zFragAsset*))").unwrap();
        assert_eq!(sig.name, "zShrapnel_DestructObjInit");
        assert_eq!(sig.arguments.len(), 4);
        assert_eq!(sig.arguments[0].base_type, "zShrapnelAsset");
        assert_eq!(sig.arguments[0].pointer_depth, 1);
        assert!(sig.arguments[0].is_pointer);

        assert_eq!(sig.arguments[1].base_type, "xModelInstance");
        assert_eq!(sig.arguments[1].pointer_depth, 1);
        assert!(sig.arguments[1].is_pointer);

        assert_eq!(sig.arguments[2].base_type, "xVec3");
        assert_eq!(sig.arguments[2].pointer_depth, 1);
        assert!(sig.arguments[2].is_pointer);

        assert_eq!(sig.arguments[3].base_type, "void (*)(zFrag*, zFragAsset*)");
        assert_eq!(sig.arguments[3].pointer_depth, 0);
        assert!(sig.arguments[3].is_pointer);
    }

    #[test]
    fn test_template_types() {
        let sig = parse_cpp_signature("func(std::vector<int>*, const std::map<std::string, int>&)").unwrap();
        assert_eq!(sig.arguments.len(), 2);
        
        assert_eq!(sig.arguments[0].base_type, "std::vector<int>");
        assert!(sig.arguments[0].is_pointer);
        
        assert_eq!(sig.arguments[1].base_type, "std::map<std::string, int>");
        assert!(sig.arguments[1].is_pointer);
    }

    #[test]
    fn test_function_pointer() {
        let sig = parse_cpp_signature("callback_func(int (*)(int, int), void*)").unwrap();
        assert_eq!(sig.arguments.len(), 2);
        
        assert_eq!(sig.arguments[0].base_type, "int (*)(int, int)");
        assert!(sig.arguments[0].is_pointer);
        
        assert_eq!(sig.arguments[1].base_type, "void");
        assert!(sig.arguments[1].is_pointer);
    }

    #[test]
    fn test_complex_types() {
        let sig = parse_cpp_signature("complex_func(const MyClass::NestedType<int>**, std::function<void(int)>&)").unwrap();
        assert_eq!(sig.arguments.len(), 2);
        
        assert_eq!(sig.arguments[0].base_type, "MyClass::NestedType<int>");
        assert_eq!(sig.arguments[0].pointer_depth, 2);
        
        assert_eq!(sig.arguments[1].base_type, "std::function<void(int)>");
        assert!(sig.arguments[1].is_pointer);
    }

    #[test]
    fn test_no_args() {
        let sig = parse_cpp_signature("empty_func()").unwrap();
        assert_eq!(sig.name, "empty_func");
        assert_eq!(sig.arguments.len(), 0);
    }

    #[test]
    fn test_primitive_types() {
        let sig = parse_cpp_signature("primitive_func(int, float, char)").unwrap();
        assert_eq!(sig.name, "primitive_func");
        assert_eq!(sig.arguments.len(), 3);
        
        assert_eq!(sig.arguments[0].base_type, "int");
        assert_eq!(sig.arguments[1].base_type, "float");
        assert_eq!(sig.arguments[2].base_type, "char");
    }
}