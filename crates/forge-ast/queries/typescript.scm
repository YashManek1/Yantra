(function_declaration name: (identifier) @name) @symbol
(method_definition name: (property_identifier) @name) @symbol
(class_declaration name: (type_identifier) @name) @symbol
(interface_declaration name: (type_identifier) @name) @symbol
(type_alias_declaration name: (type_identifier) @name) @symbol
(lexical_declaration) @symbol
(import_statement) @import
(call_expression function: (_) @callee) @call
