# frozen_string_literal: true

# parser gem の字句解析器 (`lib/parser/lexer-F1.rb`、Ragel が lexer.rl から生成したもの) の実行部を
# Rust へ写す。`generate.rb` から呼ぶ。
#
# Ragel の生成物は、遷移表 (`generate.rb` が別に書き出す) と、遷移に付いた動作の `case` 文から成る。
# 動作の番号は表と対になっているので、`case` 文を手で写すと番号を取り違える余地が残る。ここでは
# 生成物の Ruby を Prism で読み、使われている構文 (代入・比較・if・case・Ragel の goto など) を
# 1 つずつ Rust の文へ写す。字句解析器の補助メソッド (`emit`・`tok` など) は Rust 側に同じ名前で
# 手で書いたものを呼ぶ。知らない構文やメソッドに出会ったら止まるので、gem の更新で動作が変われば
# ここに対応を足すまで生成できない。

require "prism"

module LegacyLexerTranslate
  # 動作の中で使うローカル変数の型。`p` などの位置は Ruby では負になり得るので i64。
  LOCALS = {
    "p" => :int, "pe" => :int, "eof" => :int, "tm" => :int, "heredoc_e" => :int,
    "new_herebody_s" => :int, "ident_ts" => :int, "ident_te" => :int, "dots_te" => :int,
    "next_state" => :int, "cmd_state" => :bool, "followed_by_nl" => :bool,
    "nl_emitted" => :bool, "indent" => :bool, "dedent_body" => :bool,
    "ident_tok" => :str, "type" => :str, "delimiter" => :str, "digits" => :str,
    "gvar_name" => :str, "ident" => :str, "diag_msg" => :sym, "_goto_level" => :int
  }.freeze

  IVARS = {
    "@te" => :int, "@ts" => :int, "@cs" => :int, "@act" => :int, "@num_base" => :int,
    "@num_digits_s" => :int, "@num_suffix_s" => :int, "@newline_s" => :int,
    "@sharp_s" => :int, "@eq_begin_s" => :int, "@paren_nest" => :int, "@top" => :int,
    "@cs_before_block_comment" => :int, "@version" => :int, "@command_start" => :bool
  }.freeze

  GOTO_LEVELS = {
    "_resume" => "L_RESUME", "_eof_trans" => "L_EOF_TRANS", "_again" => "L_AGAIN",
    "_test_eof" => "L_TEST_EOF", "_out" => "L_OUT"
  }.freeze

  NUM_XFRM = {
    "@emit_integer" => "Integer", "@emit_rational" => "Rational",
    "@emit_imaginary" => "Imaginary", "@emit_imaginary_rational" => "ImaginaryRational",
    "@emit_integer_re" => "IntegerRe", "@emit_integer_if" => "IntegerIf",
    "@emit_integer_rescue" => "IntegerRescue", "@emit_float" => "Float",
    "@emit_imaginary_float" => "ImaginaryFloat", "@emit_float_if" => "FloatIf",
    "@emit_float_rescue" => "FloatRescue"
  }.freeze

  TABLES = {
    "PUNCTUATION" => "Table::Punctuation", "PUNCTUATION_BEGIN" => "Table::PunctuationBegin",
    "KEYWORDS" => "Table::Keywords", "KEYWORDS_BEGIN" => "Table::KeywordsBegin"
  }.freeze

  class Error < StandardError; end

  class Translator
    def initialize(messages)
      @messages = messages
      @out = +""
    end

    attr_reader :out

    def fail!(node, why)
      raise Error, "#{why}: #{node.class.name.split("::").last} `#{node.slice[0, 120]}`"
    end

    # ----- 文 -----

    def stmts(node, depth)
      return if node.nil?

      list = node.is_a?(Prism::StatementsNode) ? node.body : [node]
      list.each { |child| stmt(child, depth) }
    end

    def line(depth, text)
      @out << ("    " * depth) << text << "\n"
    end

    def stmt(node, depth)
      case node
      when Prism::BeginNode
        fail!(node, "rescue 付きの begin") if node.rescue_clause || node.ensure_clause || node.else_clause
        line(depth, "{")
        stmts(node.statements, depth + 1)
        line(depth, "}")
      when Prism::ParenthesesNode
        stmts(node.body, depth)
      when Prism::NextNode
        line(depth, "continue 'exec;")
      when Prism::LocalVariableWriteNode
        name = node.name.to_s
        if node.value.is_a?(Prism::LocalVariableWriteNode)
          # `type = delimiter = tok[0].chr`
          stmt(node.value, depth)
          line(depth, "#{local(name)} = #{local(node.value.name.to_s)}.clone();")
        elsif name == "_goto_level"
          level = node.value.is_a?(Prism::LocalVariableReadNode) && GOTO_LEVELS[node.value.name.to_s]
          fail!(node, "_goto_level の値") unless level
          line(depth, "goto_level = #{level};")
        else
          line(depth, "#{local(name)} = #{expr(node.value, LOCALS.fetch(name) { fail!(node, "知らないローカル変数 #{name}") })};")
        end
      when Prism::LocalVariableOperatorWriteNode
        line(depth, "#{local(node.name.to_s)} #{node.binary_operator}= #{expr(node.value, :int)};")
      when Prism::InstanceVariableWriteNode
        ivar_write(node, depth)
      when Prism::InstanceVariableOperatorWriteNode
        line(depth, "self.#{ivar_field(node.name.to_s)} #{node.binary_operator}= #{expr(node.value, :int)};")
      when Prism::IfNode
        if_stmt(node, depth, "if")
      when Prism::UnlessNode
        line(depth, "if !(#{expr(node.predicate, :bool)}) {")
        stmts(node.statements, depth + 1)
        if node.else_clause
          line(depth, "} else {")
          stmts(node.else_clause.statements, depth + 1)
        end
        line(depth, "}")
      when Prism::CaseNode
        case_stmt(node, depth)
      when Prism::MultiWriteNode
        multi_write(node, depth)
      when Prism::CallOrWriteNode
        # `@strings.herebody_s ||= new_herebody_s`
        fail!(node, "||= の対象") unless node.receiver&.slice == "@strings" && node.read_name.to_s == "herebody_s"
        line(depth, "if self.strings.herebody_s.is_none() {")
        line(depth + 1, "self.strings.herebody_s = Some(#{expr(node.value, :int)});")
        line(depth, "}")
      when Prism::CallNode
        call_stmt(node, depth)
      when Prism::NilNode
        # 何もしない
      else
        fail!(node, "知らない文")
      end
    end

    def if_stmt(node, depth, keyword)
      line(depth, "#{keyword} #{expr(node.predicate, :bool)} {")
      stmts(node.statements, depth + 1)
      sub = node.subsequent
      case sub
      when nil
        line(depth, "}")
      when Prism::ElseNode
        line(depth, "} else {")
        stmts(sub.statements, depth + 1)
        line(depth, "}")
      when Prism::IfNode
        @out << ("    " * depth) << "} else "
        if_stmt_inline(sub, depth)
      else
        fail!(node, "知らない else")
      end
    end

    def if_stmt_inline(node, depth)
      @out << "if #{expr(node.predicate, :bool)} {\n"
      stmts(node.statements, depth + 1)
      sub = node.subsequent
      case sub
      when nil
        line(depth, "}")
      when Prism::ElseNode
        line(depth, "} else {")
        stmts(sub.statements, depth + 1)
        line(depth, "}")
      when Prism::IfNode
        @out << ("    " * depth) << "} else "
        if_stmt_inline(sub, depth)
      end
    end

    def case_stmt(node, depth)
      fail!(node, "case の対象") unless node.predicate.slice.strip == "@act"
      line(depth, "match self.act {")
      node.conditions.each do |w|
        values = w.conditions.map { |c| expr(c, :int) }
        line(depth + 1, "#{values.join(" | ")} => {")
        stmts(w.statements, depth + 2)
        line(depth + 1, "}")
      end
      line(depth + 1, "_ => {")
      stmts(node.else_clause&.statements, depth + 2)
      line(depth + 1, "}")
      line(depth, "}")
    end

    def multi_write(node, depth)
      targets = node.lefts.map { |t| t.name.to_s }
      value = node.value
      if value.is_a?(Prism::ArrayNode)
        fail!(node, "多重代入の数") unless value.elements.size == targets.size
        temps = value.elements.each_with_index.map do |element, index|
          temp = "multi_#{index}"
          line(depth, "let #{temp} = #{expr(element, LOCALS.fetch(targets[index]))};")
          temp
        end
        targets.each_with_index { |target, index| line(depth, "#{local(target)} = #{temps[index]};") }
      else
        # `p, next_state = @strings.advance(p)` など、2 要素を返す呼び出し。
        line(depth, "let (multi_0, multi_1) = #{expr(value, :pair)};")
        line(depth, "#{local(targets[0])} = multi_0;")
        line(depth, "#{local(targets[1])} = multi_1;")
      end
    end

    def ivar_write(node, depth)
      name = node.name.to_s
      value = node.value
      case name
      when "@num_xfrm"
        xfrm = NUM_XFRM[value.slice.strip] or fail!(node, "知らない @num_xfrm")
        line(depth, "self.num_xfrm = NumXfrm::#{xfrm};")
      when "@ts"
        if value.is_a?(Prism::NilNode)
          line(depth, "self.ts = NIL;")
        else
          line(depth, "self.ts = #{expr(value, :int)};")
        end
      when "@cs"
        line(depth, "self.cs = #{expr(value, :int)};")
      else
        type = IVARS.fetch(name) { fail!(node, "知らないインスタンス変数 #{name}") }
        line(depth, "self.#{ivar_field(name)} = #{expr(value, type)};")
      end
    end

    def call_stmt(node, depth)
      receiver = node.receiver
      name = node.name.to_s
      if receiver&.slice == "@stack" && name == "[]="
        index, value = node.arguments.arguments
        line(depth, "self.stack_set(#{expr(index, :int)}, #{expr(value, :int)});")
        return
      end
      if receiver.nil? && name == "diagnostic"
        diagnostic(node, depth)
        return
      end
      if receiver.nil? && name == "check_ambiguous_slash"
        # warning だけを出すメソッド。Lint/Syntax は warning を報告しない。
        return
      end
      text = call_expr(node, :stmt)
      line(depth, "#{text};")
    end

    # `diagnostic level, reason, arguments, location`。warning は Lint/Syntax が報告しないので捨てる。
    # fatal は parser gem が `Parser::SyntaxError` を投げるので、字句解析をそこで打ち切る。
    def diagnostic(node, depth)
      args = node.arguments.arguments
      level = args[0].unescaped
      return if level == "warning"

      reason_node = args[1]
      message =
        if reason_node.is_a?(Prism::SymbolNode)
          template = @messages.fetch(reason_node.unescaped.to_sym)
          arguments = args[2].is_a?(Prism::HashNode) ? args[2] : nil
          format_message(template, arguments)
        else
          # `diagnostic :error, diag_msg, { name: ... }, ...` (ivar_name / cvar_name)
          "self.message_for(#{expr(reason_node, :sym)}, &#{hash_args(args[2])})"
        end
      location = args[3] ? expr(args[3], :range) : "(self.ts, self.te)"
      rust_level = level == "fatal" ? "Level::Fatal" : "Level::Error"
      line(depth, "self.diagnostic(#{rust_level}, #{message}, #{location});")
      line(depth, "return Err(Fatal);") if level == "fatal"
    end

    def hash_args(node)
      return "[]" if node.nil? || node.is_a?(Prism::NilNode)

      pairs = node.elements.map do |assoc|
        "(\"#{assoc.key.unescaped}\", #{expr(assoc.value, :str)})"
      end
      "[#{pairs.join(", ")}]"
    end

    def format_message(template, arguments)
      values = {}
      arguments&.elements&.each { |assoc| values[assoc.key.unescaped] = expr(assoc.value, :str) }
      if values.empty?
        "#{template.inspect}.to_owned()"
      else
        format_args = []
        text = template.gsub(/%\{(\w+)\}/) do
          format_args << values.fetch(Regexp.last_match(1))
          "{}"
        end
        "format!(#{text.inspect}, #{format_args.join(", ")})"
      end
    end

    # ----- 式 -----

    def local(name)
      name == "type" ? "type_" : name
    end

    def ivar_field(name)
      name.delete_prefix("@")
    end

    def expr(node, want)
      case node
      when Prism::IntegerNode
        node.value.to_s
      when Prism::TrueNode
        "true"
      when Prism::FalseNode
        "false"
      when Prism::NilNode
        case want
        when :value then "TokenValue::Nil"
        when :int then "NIL"
        else fail!(node, "nil の型 #{want}")
        end
      when Prism::ParenthesesNode
        body = node.body.body
        fail!(node, "括弧の中の文の数") unless body.size == 1
        "(#{expr(body[0], want)})"
      when Prism::LocalVariableReadNode
        name = node.name.to_s
        type = LOCALS.fetch(name) { fail!(node, "知らないローカル変数 #{name}") }
        # 文字列のローカル変数は値で渡す先があるので、読むたびに複製する。
        GOTO_LEVELS[name] || convert(type == :str ? "#{local(name)}.clone()" : local(name), type, want, node)
      when Prism::InstanceVariableReadNode
        name = node.name.to_s
        type = IVARS.fetch(name) { fail!(node, "知らないインスタンス変数 #{name}") }
        convert("self.#{ivar_field(name)}", type, want, node)
      when Prism::SymbolNode
        sym = node.unescaped
        if want == :tok || sym.start_with?("t") || sym.start_with?("k")
          "Tok::#{sym}"
        else
          "\"#{sym}\""
        end
      when Prism::StringNode
        convert("String::from(#{rust_string(node.unescaped)})", :str, want, node)
      when Prism::IfNode
        # 三項演算子 `a ? b : c`
        then_value = node.statements.body.first
        else_value = node.subsequent.statements.body.first
        "(if #{expr(node.predicate, :bool)} { #{expr(then_value, want)} } else { #{expr(else_value, want)} })"
      when Prism::AndNode
        "(#{expr(node.left, :bool)} && #{expr(node.right, :bool)})"
      when Prism::OrNode
        "(#{expr(node.left, :bool)} || #{expr(node.right, :bool)})"
      when Prism::CallNode
        call_expr(node, want)
      when Prism::NumberedReferenceReadNode
        convert("captures[#{node.number}].clone()", :str, want, node)
      else
        fail!(node, "知らない式")
      end
    end

    def rust_string(text)
      escaped = text.gsub("\\", "\\\\\\\\").gsub('"', '\\"').gsub("\n", "\\n").gsub("\r", "\\r").gsub("\t", "\\t")
      "\"#{escaped}\""
    end

    # 型を合わせる。`:value` (emit に渡す値) と `:str` の間の変換もここで行う。
    def convert(text, have, want, node)
      return text if want == have || want == :stmt || want == :any

      case [have, want]
      when [:str, :value] then "TokenValue::from(#{text}.as_str())"
      when [:int, :value] then "TokenValue::from(#{text}.to_string().as_str())"
      when [:sym, :str] then "#{text}.to_owned()"
      else
        fail!(node, "型 #{have} を #{want} へ変換できない")
      end
    end

    BINARY = %w[+ - == != >= <= > < %].freeze

    def call_expr(node, want)
      receiver = node.receiver
      name = node.name.to_s
      args = node.arguments ? node.arguments.arguments : []

      # 二項演算
      if receiver && BINARY.include?(name) && args.size == 1
        if %w[== !=].include?(name)
          left_type = type_of(receiver)
          right = expr(args[0], left_type == :value ? :value : left_type)
          return convert("(#{expr(receiver, left_type)} #{name} #{right})", :bool, want, node)
        end
        if name == "+" && type_of(receiver) == :str
          return convert("format!(\"{}{}\", #{expr(receiver, :str)}, #{expr(args[0], :str)})", :str, want, node)
        end
        type = %w[>= <= > <].include?(name) ? :bool : :int
        return convert("(#{expr(receiver, :int)} #{name} #{expr(args[0], :int)})", type, want, node)
      end
      if receiver && name == "!" && args.empty?
        return convert("!(#{expr(receiver, :bool)})", :bool, want, node)
      end

      key = receiver ? "#{receiver.slice.strip}.#{name}" : name
      text, type =
        case key
        when "emit"
          emit_call(args)
        when "emit_table"
          table = TABLES.fetch(args[0].slice) { fail!(node, "知らない表") }
          s = args[1] ? expr(args[1], :int) : "self.ts"
          e = args[2] ? expr(args[2], :int) : "self.te"
          ["self.emit_table(#{table}, #{s}, #{e})", :stmt]
        when "emit_do"
          ["self.emit_do(#{args[0] ? expr(args[0], :bool) : "false"})", :stmt]
        when "tok"
          s = args[0] ? expr(args[0], :int) : "self.ts"
          e = args[1] ? expr(args[1], :int) : "self.te"
          ["self.tok(#{s}, #{e})", :str]
        when "range"
          s = args[0] ? expr(args[0], :int) : "self.ts"
          e = args[1] ? expr(args[1], :int) : "self.te"
          ["(#{s}, #{e})", :range]
        when "version?"
          ["self.version_in(&[#{args.map { |a| expr(a, :int) }.join(", ")}])", :bool]
        when "arg_or_cmdarg"
          ["self.arg_or_cmdarg(#{expr(args[0], :bool)})", :int]
        when "on_newline"
          ["self.on_newline(#{expr(args[0], :int)})", :int]
        when "emit_comment_from_range"
          ["self.emit_comment_from_range(#{expr(args[0], :int)}, #{expr(args[1], :int)})", :stmt]
        when "emit_comment"
          ["self.emit_comment(#{expr(args[0], :int)}, #{expr(args[1], :int)})", :stmt]
        when "numeric_literal_int"
          ["self.numeric_literal_int()", :str]
        when "@num_xfrm.call"
          ["self.num_xfrm_call(#{expr(args[0], :str)}, #{expr(args[1], :int)})", :int]
        when "Float"
          [expr(args[0], :str), :str]
        when "stack_pop"
          ["self.stack_pop()", :int]
        when "e_lbrace", "emit_singleton_class", "emit_rbrace_rparen_rbrack",
             "emit_global_var", "emit_class_var", "emit_instance_var"
          fail!(node, "引数付きの #{name}") unless args.empty?
          ["self.#{name}()", :stmt]
        when "emit_colon_with_digits"
          ["self.emit_colon_with_digits(#{expr(args[0], :int)}, #{expr(args[1], :int)}, #{expr(args[2], :sym)})", :int]
        when "@static_env.nil?"
          ["false", :bool]
        when "@static_env.declared?"
          ["self.static_env.is_declared(&#{expr(args[0], :str)})", :bool]
        when "@lambda_stack.last"
          ["self.lambda_stack.last().copied().unwrap_or(NIL)", :int]
        when "@lambda_stack.pop"
          ["self.lambda_stack.pop()", :stmt]
        when "@lambda_stack.push"
          ["self.lambda_stack.push(#{expr(args[0], :int)})", :stmt]
        when "@lambda_stack.any?"
          ["!self.lambda_stack.is_empty()", :bool]
        when "@cond.push", "@cmdarg.push"
          ["self.#{receiver.slice.delete_prefix("@")}.push(#{expr(args[0], :bool)})", :stmt]
        when "@cond.active?"
          ["self.cond.is_active()", :bool]
        when "@strings.push_literal"
          values = args.each_with_index.map do |arg, index|
            index < 2 ? expr(arg, :str) : index == 3 ? opt_int(arg) : index > 3 ? expr(arg, :bool) : expr(arg, :int)
          end
          values << "None" if values.size < 4
          values << "false" while values.size < 7
          ["self.push_literal(#{values.join(", ")})", :stmt]
        when "@strings.herebody_s"
          ["self.strings.herebody_s.unwrap_or(NIL)", :int]
        when "@strings.close_interp_on_current_literal"
          ["self.close_interp_on_current_literal(#{expr(args[0], :int)})", :bool]
        when "@strings.advance"
          ["self.strings_advance(#{expr(args[0], :int)})?", :pair]
        when "@strings.read_character_constant"
          ["self.read_character_constant(#{expr(args[0], :int)})?", :pair]
        when "@token_queue.empty?"
          ["self.token_queue.is_empty()", :bool]
        when "@source_buffer.slice"
          ["self.tok(#{expr(args[0], :int)}, #{expr(args[0], :int)} + #{expr(args[1], :int)})", :str]
        when "@context.in_argdef", "@context.in_kwarg"
          ["self.context.#{name}", :bool]
        else
          string_method(node, receiver, name, args) || fail!(node, "知らない呼び出し #{key}")
        end
      convert(text, type, want, node)
    end

    def opt_int(node)
      node.is_a?(Prism::NilNode) ? "None" : "Some(#{expr(node, :int)})"
    end

    # 文字列への操作。
    def string_method(node, receiver, name, args)
      return nil unless receiver

      case name
      when "freeze"
        [expr(receiver, :str), :str]
      when "to_i", "to_f"
        # 数値リテラルの値は診断に使わないので、字句のまま持つ。
        [expr(receiver, :str), :str]
      when "chr"
        [expr(receiver, :str), :str]
      when "[]"
        arg = args[0]
        if arg.is_a?(Prism::RangeNode)
          ["str_slice(&#{expr(receiver, :str)}, #{expr(arg.left, :int)}, #{expr(arg.right, :int)})", :str]
        else
          ["str_char_at(&#{expr(receiver, :str)}, #{expr(arg, :int)})", :str]
        end
      when "count"
        ["(#{expr(receiver, :str)}.matches(#{expr(args[0], :str)}.as_str()).count() as i64)", :int]
      when "end_with?"
        ["#{expr(receiver, :str)}.ends_with(#{expr(args[0], :str)}.as_str())", :bool]
      when "start_with?"
        ["#{expr(receiver, :str)}.starts_with(#{expr(args[0], :str)}.as_str())", :bool]
      when "rstrip"
        ["#{expr(receiver, :str)}.trim_end().to_owned()", :str]
      when "length"
        ["(#{expr(receiver, :str)}.chars().count() as i64)", :int]
      when "empty?"
        ["#{expr(receiver, :str)}.is_empty()", :bool]
      when "inspect"
        ["ruby_inspect(&#{expr(receiver, :str)})", :str]
      when "=~"
        regexp = args[0]
        fail!(node, "=~ の右辺") unless regexp.is_a?(Prism::RegularExpressionNode)
        case regexp.unescaped
        when '^<<(-?)(~?)(["\'`]?)(.*)\3$'
          ["{ captures = heredoc_captures(&#{expr(receiver, :str)}); true }", :bool]
        when '\A_[1-9]\z'
          ["is_numparam_name(&#{expr(receiver, :str)})", :bool]
        when "[A-Z]"
          ["#{expr(receiver, :str)}.starts_with(|c: char| c.is_ascii_uppercase())", :bool]
        else
          fail!(node, "知らない正規表現 #{regexp.unescaped}")
        end
      end
    end

    def emit_call(args)
      tok = args[0].is_a?(Prism::SymbolNode) ? "Tok::#{args[0].unescaped}" : nil
      if tok.nil?
        # 1.8 の分岐 (`emit((... =~ /[A-Z]/) ? :tCONSTANT : :tIDENTIFIER, ...)`) は対象外の版。
        return ["unreachable!(\"1.8 の分岐\")", :stmt]
      end
      value = args[1] ? expr(args[1], :value) : "TokenValue::from(self.tok(self.ts, self.te).as_str())"
      s = args[2] ? expr(args[2], :int) : "self.ts"
      e = args[3] ? expr(args[3], :int) : "self.te"
      ["self.emit(#{tok}, #{value}, #{s}, #{e})", :stmt]
    end

    def type_of(node)
      case node
      when Prism::LocalVariableReadNode then LOCALS.fetch(node.name.to_s, :int)
      when Prism::InstanceVariableReadNode then IVARS.fetch(node.name.to_s, :int)
      when Prism::StringNode then :str
      when Prism::IntegerNode then :int
      when Prism::CallNode
        case node.name.to_s
        when "tok", "freeze", "chr", "[]", "rstrip", "inspect", "slice" then :str
        when "last" then :int
        else
          node.receiver&.slice == "@source_buffer" ? :str : :int
        end
      else :int
      end
    end
  end

  # 本体の字句解析器の `advance` の実行部。
  def self.translate_main(path, messages)
    src = File.read(path)
    result = Prism.parse(src)
    raise Error, "#{path} を読めない" unless result.success?

    cases = {}
    stack = [result.value]
    until stack.empty?
      node = stack.pop
      next unless node

      if node.is_a?(Prism::CaseNode) && node.predicate
        section = case node.predicate.slice
                  when /_lex_trans_actions\[_trans\]/ then :trans
                  when /_lex_from_state_actions\[ ?@cs\]/ then :from
                  when /_lex_to_state_actions\[ ?@cs\]/ then :to
                  end
        cases[section] = node if section
      end
      stack.concat(node.compact_child_nodes)
    end
    raise Error, "動作の case 文が見つからない" unless cases.keys.sort == %i[from to trans]

    emitter = Translator.new(messages)
    emit_switch = lambda do |case_node, depth|
      emitter.line(depth, "match #{case_node == cases[:trans] ? "TRANS_ACTIONS[trans]" : (case_node == cases[:from] ? "FROM_STATE_ACTIONS[self.cs as usize]" : "TO_STATE_ACTIONS[self.cs as usize]")} {")
      case_node.conditions.each do |w|
        numbers = w.conditions.map(&:slice)
        emitter.line(depth + 1, "#{numbers.join(" | ")} => {")
        emitter.stmts(w.statements, depth + 2)
        emitter.line(depth + 1, "}")
      end
      emitter.line(depth + 1, "_ => {}")
      emitter.line(depth, "}")
    end

    emitter.line(0, "impl Lexer {")
    emitter.line(1, "/// `Parser::Lexer#advance` の `%% write exec;` の部分。")
    emitter.line(1, "#[allow(clippy::all, unused_assignments, unused_mut, unused_variables, unreachable_code, unused_parens, non_snake_case)]")
    emitter.line(1, "pub(crate) fn exec_main(&mut self, mut p: i64, pe: i64, eof: i64, mut cmd_state: bool) -> Result<i64, Fatal> {")
    emitter.line(2, "let mut goto_level = 0;")
    emitter.line(2, "let mut trans: usize = 0;")
    LOCALS.each do |name, type|
      next if %w[p pe eof cmd_state _goto_level].include?(name)

      init = { int: "0i64", bool: "false", str: "String::new()", sym: "\"\"" }.fetch(type)
      emitter.line(2, "let mut #{emitter.local(name)} = #{init};")
    end
    emitter.line(2, "let mut captures: [String; 5] = Default::default();")
    emitter.line(2, "'exec: loop {")
    emitter.line(3, "if goto_level <= 0 {")
    emitter.line(4, "if p == pe { goto_level = L_TEST_EOF; continue 'exec; }")
    emitter.line(4, "if self.cs == 0 { goto_level = L_OUT; continue 'exec; }")
    emitter.line(3, "}")
    emitter.line(3, "if goto_level <= L_RESUME {")
    emit_switch.call(cases[:from], 4)
    emitter.line(4, "trans = self.transition(p);")
    emitter.line(3, "}")
    emitter.line(3, "if goto_level <= L_EOF_TRANS {")
    emitter.line(4, "self.cs = i64::from(TRANS_TARGS[trans]);")
    emitter.line(4, "if TRANS_ACTIONS[trans] != 0 {")
    emit_switch.call(cases[:trans], 5)
    emitter.line(4, "}")
    emitter.line(3, "}")
    emitter.line(3, "if goto_level <= L_AGAIN {")
    emit_switch.call(cases[:to], 4)
    emitter.line(4, "if self.cs == 0 { goto_level = L_OUT; continue 'exec; }")
    emitter.line(4, "p += 1;")
    emitter.line(4, "if p != pe { goto_level = L_RESUME; continue 'exec; }")
    emitter.line(3, "}")
    emitter.line(3, "if goto_level <= L_TEST_EOF && p == eof && EOF_TRANS[self.cs as usize] > 0 {")
    emitter.line(4, "trans = usize::from(EOF_TRANS[self.cs as usize]) - 1;")
    emitter.line(4, "goto_level = L_EOF_TRANS;")
    emitter.line(4, "continue 'exec;")
    emitter.line(3, "}")
    emitter.line(3, "break 'exec;")
    emitter.line(2, "}")
    emitter.line(2, "Ok(p)")
    emitter.line(1, "}")
    emitter.line(0, "}")
    emitter.out
  end
end
