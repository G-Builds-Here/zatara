; zatara-netcoredbg — C# gutter runnables
;
; Contract (epic LOC-0103 business rules, DR-5):
;   * The tag strings `csharp-test-method` and `csharp-test-class` are the 3-way
;     join key between this query, tasks.json, and the DAP locator. Do not rename.
;   * Captures @csharp_namespace / @csharp_class_name / @csharp_method_name /
;     @csharp_case_args are exposed by Zed as ${ZED_CUSTOM_...} task variables
;     and compose the exact FullyQualifiedName= filter (never a bare name).
;   * `_`-prefixed captures are match-only (Zed does not expose them).
;   * File-scoped namespaces: Zed's pinned grammar (tree-sitter-c-sharp @485f0ba)
;     parses `namespace X;` with members as SIBLINGS of the namespace node — the
;     `.` sibling operator below is what closes the reference prototype's gap.
;   * Boundary: classes at the global namespace (no file-scoped or block
;     namespace) intentionally get no icons — an exact-FQN filter cannot be
;     composed from captures without a namespace, and a silently-wrong filter
;     is the defect class this epic kills. Global-namespace breadth is a
;     framework-breadth story (LOC-0113), not a guess here.

; ============================================================
; Test method (block namespace) — xunit / nunit / mstest
; ============================================================
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

; ============================================================
; Test method (file-scoped namespace)
; ============================================================
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   .
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

; ============================================================
; Parameterized cases — one gutter action per [TestCase]/[DataRow]/
; [InlineData] attribute, tagged by FRAMEWORK because each adapter's
; case filter grammar differs (live probes 2026-10-12, see tasks.json):
;   nunit   FQN includes the no-space arg body   -> FQN~body
;   mstest  FQN excludes args, Name is the row   -> Name~body
;   xunit   args only in the named DisplayName   -> DisplayName~p: v, …
; Arguments are captured one-by-one (integer_literal only, arity 3 —
; the committed fixture matrix); a case outside that shape gets NO case
; icon, never a guessed filter (same rule as the global-namespace
; boundary above). Method-level icons above still run all cases.
; The task templates for these tags compose the filters; the runner's
; build_filter is the debug-path equivalent.
; ============================================================

; ---- nunit [TestCase] -------------------------------------------------
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
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   .
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

; ---- mstest [DataRow] --------------------------------------------------
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
   .
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

; ---- xunit [InlineData] -------------------------------------------------
; Also captures the method's formal parameter names: xunit's DisplayName
; names its arguments (`a: 1, b: 2`), so the case filter must zip
; parameters over arguments — the template does the zip, scm cannot.
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
                (_
                  (_ (integer_literal) @csharp_case_arg_1)
                  (_ (integer_literal) @csharp_case_arg_2)
                  (_ (integer_literal) @csharp_case_arg_3)) @run)
              )
            name: (identifier) @csharp_method_name
            parameters: (parameter_list
              (parameter name: (identifier) @csharp_method_param_1)
              (parameter name: (identifier) @csharp_method_param_2)
              (parameter name: (identifier) @csharp_method_param_3)))))))
  (#set! tag csharp-test-case-xunit)
)

(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   .
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "InlineData")
             (_
               (_ (integer_literal) @csharp_case_arg_1)
               (_ (integer_literal) @csharp_case_arg_2)
               (_ (integer_literal) @csharp_case_arg_3)) @run)
           )
         name: (identifier) @csharp_method_name
         parameters: (parameter_list
           (parameter name: (identifier) @csharp_method_param_1)
           (parameter name: (identifier) @csharp_method_param_2)
           (parameter name: (identifier) @csharp_method_param_3))))))
  (#set! tag csharp-test-case-xunit)
)

; ============================================================
; Test class by attribute: nunit [TestFixture], mstest [TestClass]
; (block namespace)
; ============================================================
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

; ============================================================
; Test class by name ending with "Test"/"Tests" (block namespace)
; — xunit has no class attribute; naming convention is the marker.
; ============================================================
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @run @csharp_class_name
        (#match? @csharp_class_name "Tests?$"))))
  (#set! tag csharp-test-class)
)

; ============================================================
; Test class by name ending with "Test"/"Tests" (file-scoped namespace)
; ============================================================
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   .
   (class_declaration
     name: (identifier) @run @csharp_class_name
     (#match? @csharp_class_name "Tests?$")))
  (#set! tag csharp-test-class)
)
