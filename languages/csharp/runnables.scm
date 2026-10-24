(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "InlineData")
                (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]")
              )
            name: (identifier) @csharp_method_name
            parameters: (parameter_list
              . (parameter name: (identifier) @csharp_method_param_1) .))))))
  (#set! tag csharp-test-case-generic-1)
)
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "InlineData")
             (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]")
           )
         name: (identifier) @csharp_method_name
         parameters: (parameter_list
           . (parameter name: (identifier) @csharp_method_param_1) .)))))
  (#set! tag csharp-test-case-generic-1)
)
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "InlineData")
                (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_2 @csharp_case_str_2) @csharp_case_raw_2)) (_ (_) @csharp_case_arg_2 @csharp_case_raw_2)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]") (#not-match? @csharp_case_arg_2 "^[@\"]")
              )
            name: (identifier) @csharp_method_name
            parameters: (parameter_list
              . (parameter name: (identifier) @csharp_method_param_1)
              . (parameter name: (identifier) @csharp_method_param_2) .))))))
  (#set! tag csharp-test-case-generic-2)
)
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "InlineData")
             (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_2 @csharp_case_str_2) @csharp_case_raw_2)) (_ (_) @csharp_case_arg_2 @csharp_case_raw_2)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]") (#not-match? @csharp_case_arg_2 "^[@\"]")
           )
         name: (identifier) @csharp_method_name
         parameters: (parameter_list
           . (parameter name: (identifier) @csharp_method_param_1)
              . (parameter name: (identifier) @csharp_method_param_2) .)))))
  (#set! tag csharp-test-case-generic-2)
)
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "InlineData")
                (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_2 @csharp_case_str_2) @csharp_case_raw_2)) (_ (_) @csharp_case_arg_2 @csharp_case_raw_2)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_3 @csharp_case_str_3) @csharp_case_raw_3)) (_ (_) @csharp_case_arg_3 @csharp_case_raw_3)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]") (#not-match? @csharp_case_arg_2 "^[@\"]") (#not-match? @csharp_case_arg_3 "^[@\"]")
              )
            name: (identifier) @csharp_method_name
            parameters: (parameter_list
              . (parameter name: (identifier) @csharp_method_param_1)
              . (parameter name: (identifier) @csharp_method_param_2)
              . (parameter name: (identifier) @csharp_method_param_3) .))))))
  (#set! tag csharp-test-case-generic-3)
)
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "InlineData")
             (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_2 @csharp_case_str_2) @csharp_case_raw_2)) (_ (_) @csharp_case_arg_2 @csharp_case_raw_2)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_3 @csharp_case_str_3) @csharp_case_raw_3)) (_ (_) @csharp_case_arg_3 @csharp_case_raw_3)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]") (#not-match? @csharp_case_arg_2 "^[@\"]") (#not-match? @csharp_case_arg_3 "^[@\"]")
           )
         name: (identifier) @csharp_method_name
         parameters: (parameter_list
           . (parameter name: (identifier) @csharp_method_param_1)
              . (parameter name: (identifier) @csharp_method_param_2)
              . (parameter name: (identifier) @csharp_method_param_3) .)))))
  (#set! tag csharp-test-case-generic-3)
)
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "TestCase")
             (_
               (_ (integer_literal) @csharp_case_arg_1)
               (_ (integer_literal) @csharp_case_arg_2)
               (_ (integer_literal) @csharp_case_arg_3)) @run)
           )
         name: (identifier) @csharp_method_name))))
  (#set! tag csharp-test-case-nunit)
)
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "DataRow")
                (_
                  (_ (integer_literal) @csharp_case_arg_1)
                  (_ (integer_literal) @csharp_case_arg_2)
                  (_ (integer_literal) @csharp_case_arg_3)) @run)
              )
            name: (identifier) @csharp_method_name)))))
  (#set! tag csharp-test-case-mstest)
)
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "DataRow")
             (_
               (_ (integer_literal) @csharp_case_arg_1)
               (_ (integer_literal) @csharp_case_arg_2)
               (_ (integer_literal) @csharp_case_arg_3)) @run)
           )
         name: (identifier) @csharp_method_name))))
  (#set! tag csharp-test-case-mstest)
)
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        (attribute_list
          (attribute
            name: (_) @class_attribute
            (#match? @class_attribute "^(TestFixture|TestClass)$")))
        name: (identifier) @run @csharp_class_name)))
  (#set! tag csharp-test-class)
)
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (_) @attribute_name
                (#match? @attribute_name "^(Fact|Theory|Test|TestCase|TestCaseSource|TestMethod|DataTestMethod)$")))
            name: (identifier) @run @csharp_method_name)))))
  (#set! tag csharp-test-method)
)
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (_) @attribute_name
             (#match? @attribute_name "^(Fact|Theory|Test|TestCase|TestCaseSource|TestMethod|DataTestMethod)$")))
         name: (identifier) @run @csharp_method_name))))
  (#set! tag csharp-test-method)
)
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "TestCase")
                (_
                  (_ (integer_literal) @csharp_case_arg_1)
                  (_ (integer_literal) @csharp_case_arg_2)
                  (_ (integer_literal) @csharp_case_arg_3)) @run)
              )
            name: (identifier) @csharp_method_name)))))
  (#set! tag csharp-test-case-nunit)
)
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @run @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (_) @attribute_name
                (#match? @attribute_name "^(Fact|Theory|Test|TestCase|TestCaseSource|TestMethod|DataTestMethod)$"))))))))
  (#set! tag csharp-test-class)
)
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   (class_declaration
     name: (identifier) @run @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (_) @attribute_name
             (#match? @attribute_name "^(Fact|Theory|Test|TestCase|TestCaseSource|TestMethod|DataTestMethod)$")))))))
  (#set! tag csharp-test-class)
)
