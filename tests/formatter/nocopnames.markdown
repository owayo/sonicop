# RuboCop Inspection Report

4 files inspected, 6 offenses detected:

### alpha.rb - (4 offenses)
  * **Line # 1 - convention:** Missing frozen string literal comment.

    ```rb
    x = "a<b>&c"
    ```

  * **Line # 1 - convention:** Prefer single-quoted strings when you don't need string interpolation or special symbols.

    ```rb
    x = "a<b>&c"
    ```

  * **Line # 2 - convention:** Trailing whitespace detected.

    ```rb
    y = 1  
    ```

  * **Line # 3 - convention:** Trailing whitespace detected.

### beta/gamma.rb - (1 offense)
  * **Line # 3 - convention:** Use `%w` or `%W` for an array of words.

    ```rb
    z = [ ...
    ```

### tabbed.rb - (1 offense)
  * **Line # 4 - warning:** Useless assignment to variable - `value`.

    ```rb
    	value = 1
    ```

