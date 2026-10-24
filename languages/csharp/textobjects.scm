(comment)+ @comment.around
(method_declaration
  body: (_
    "{"
    (_)* @function.inside
    "}")) @function.around
(class_declaration
  body: (_
    "{"
    (_)* @class.inside
    "}")) @class.around
(method_declaration) @function.around
(lambda_expression
  body: (_
    "{"?
    (_)* @function.inside
    "}"?)) @function.around
