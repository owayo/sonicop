# RuboCop Inspection Report

5 files inspected, 10 offenses detected:

### alpha.rb - (6 offenses)
  * **Line # 1 - warning:** Lint/UselessAssignment: Useless assignment to variable - `x`.

    ```rb
    x = "a<b>&c"
    ```

  * **Line # 1 - convention:** Style/FrozenStringLiteralComment: Missing frozen string literal comment.

    ```rb
    x = "a<b>&c"
    ```

  * **Line # 1 - convention:** Style/StringLiterals: Prefer single-quoted strings when you don't need string interpolation or special symbols.

    ```rb
    x = "a<b>&c"
    ```

  * **Line # 2 - warning:** Lint/UselessAssignment: Useless assignment to variable - `y`.

    ```rb
    y = 1  
    ```

  * **Line # 2 - convention:** Layout/TrailingWhitespace: Trailing whitespace detected.

    ```rb
    y = 1  
    ```

  * **Line # 3 - convention:** Layout/TrailingWhitespace: Trailing whitespace detected.

### beta/gamma.rb - (1 offense)
  * **Line # 3 - convention:** Style/WordArray: Use `%w` or `%W` for an array of words.

    ```rb
    z = [ ...
    ```

### fmt.rb - (2 offenses)
  * **Line # 3 - convention:** Style/FormatStringToken: Prefer annotated tokens (like `%<foo>s`) over unannotated tokens (like `%s`).

    ```rb
    format('%s %s', 1, 2)
    ```

  * **Line # 3 - convention:** Style/FormatStringToken: Prefer annotated tokens (like `%<foo>s`) over unannotated tokens (like `%s`).

    ```rb
    format('%s %s', 1, 2)
    ```

### tabbed.rb - (1 offense)
  * **Line # 4 - warning:** Lint/UselessAssignment: Useless assignment to variable - `value`.

    ```rb
    	value = 1
    ```

