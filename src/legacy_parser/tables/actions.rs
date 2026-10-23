//! **`scripts/legacy_parser/generate.rb` の生成物。手で変更しない。**
//!
//! parser 3.3.12.0 (racc 1.8.1) の表と動作を読み出したもの。
//! 表を再構成せず gem から取得する理由は生成スクリプトを参照。

/// 文法動作の本文のハッシュで識別する。各項目に原文を引用する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// 動作の無い規則では最初の記号の値を返す。
    None,
    /// 使用箇所: ruby27 612 (@36), ruby30 626 (@32).
    ///
    /// ```ruby
    /// result = @context.in_kwarg
    /// @context.in_kwarg = true
    /// ```
    A00a2e939,
    /// 使用箇所: ruby27 87 (mlhs_basic), ruby27 278 (args), ruby27 285 (mrhs), ruby30 93 (mlhs_basic) and 14 more.
    ///
    /// ```ruby
    /// result = [ @builder.splat(val[0], val[1]) ]
    /// ```
    A00a4a5cc,
    /// 使用箇所: ruby27 223 (arg), ruby27 224 (arg), ruby27 236 (arg), ruby30 229 (arg) and 14 more.
    ///
    /// ```ruby
    /// result = @builder.unary_op(val[0], val[1])
    /// ```
    A00b97ef8,
    /// 使用箇所: ruby27 605 (backref), ruby30 616 (backref), ruby31 631 (backref), ruby32 629 (backref) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.back_ref(val[0])
    /// ```
    A01d65ee6,
    /// 使用箇所: ruby27 671 (f_rest_arg), ruby30 684 (f_rest_arg), ruby31 698 (f_rest_arg).
    ///
    /// ```ruby
    /// result = [ @builder.restarg(val[0]) ]
    /// ```
    A0228739a,
    /// 使用箇所: ruby31 678 (f_block_kw), ruby32 676 (f_block_kw), ruby33 677 (f_block_kw), ruby34 677 (f_block_kw).
    ///
    /// ```ruby
    /// @context.in_argdef = true
    /// result = @builder.kwoptarg(val[0], val[1])
    /// ```
    A051342d1,
    /// 使用箇所: ruby31 45 (command_asgn), ruby31 255 (arg), ruby32 45 (command_asgn), ruby32 255 (arg).
    ///
    /// ```ruby
    /// def_t, (name_t, ctx) = val[0]
    /// endless_method_name(name_t)
    /// rescue_body = @builder.rescue_body(val[4],
    /// nil, nil, nil,
    /// nil, val[5])
    /// method_body = @builder.begin_body(val[3], [ rescue_body ])
    /// result = @builder.def_endless_method(def_t, name_t,
    /// val[1], val[2], method_body)
    /// local_pop
    /// @current_arg_stack.pop
    /// @context.in_def = ctx.in_def
    /// ```
    A0534c239,
    /// 使用箇所: ruby32 298 (args).
    ///
    /// ```ruby
    /// if !@static_env.declared_anonymous_restarg?
    /// diagnostic :error, :no_anonymous_restarg, nil, val[2]
    /// end
    /// result = val[0] << @builder.forwarded_restarg(val[2])
    /// ```
    A05648330,
    /// 使用箇所: ruby27 27 (stmt), ruby30 27 (stmt), ruby31 27 (stmt), ruby32 27 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.loop_mod(:while, val[0], val[1], val[2])
    /// ```
    A05f9f431,
    /// 使用箇所: ruby27 49 (expr), ruby27 239 (arg), ruby30 49 (expr), ruby30 245 (arg) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.logical_op(:and, val[0], val[1], val[2])
    /// ```
    A073a7e79,
    /// 使用箇所: ruby27 210 (arg), ruby27 506 (p_value), ruby30 216 (arg), ruby30 518 (p_value) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.range_inclusive(val[0], val[1], val[2])
    /// ```
    A07650c53,
    /// 使用箇所: ruby31 425 (lambda_body), ruby31 427 (lambda_body), ruby32 427 (lambda_body), ruby32 429 (lambda_body) and 4 more.
    ///
    /// ```ruby
    /// @context.in_lambda = val[1].in_lambda
    /// result = [ val[0], val[2], val[3] ]
    /// ```
    A07b7b35d,
    /// 使用箇所: ruby27 466 (p_expr_basic).
    ///
    /// ```ruby
    /// @pattern_hash_keys.pop
    /// result = @builder.array_pattern(val[0], val[2], val[3])
    /// ```
    A08c12e78,
    /// 使用箇所: ruby27 482 (p_args_head), ruby30 493 (p_args_head), ruby31 503 (p_args_head), ruby32 503 (p_args_head) and 2 more.
    ///
    /// ```ruby
    /// last_item = @builder.match_with_trailing_comma(val[1], val[2])
    /// result = [ *val[0], last_item ]
    /// ```
    A08ebba77,
    /// 使用箇所: ruby27 414 (block_call), ruby27 418 (method_call), ruby27 419 (method_call), ruby30 422 (block_call) and 14 more.
    ///
    /// ```ruby
    /// lparen_t, args, rparen_t = val[3]
    /// result = @builder.call_method(val[0], val[1], val[2],
    /// lparen_t, args, rparen_t)
    /// ```
    A0981cb7d,
    /// 使用箇所: ruby27 611 (f_arglist), ruby30 624 (f_paren_args).
    ///
    /// ```ruby
    /// result = @builder.forward_only_args(val[0], val[1], val[2])
    /// @static_env.declare_forward_args
    /// @lexer.state = :expr_value
    /// ```
    A09a78472,
    /// 使用箇所: ruby27 595 (keyword_variable), ruby30 606 (keyword_variable), ruby31 621 (keyword_variable), ruby32 619 (keyword_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.true(val[0])
    /// ```
    A0a165b16,
    /// 使用箇所: ruby27 711 (term), ruby30 724 (term), ruby31 740 (term), ruby32 736 (term) and 2 more.
    ///
    /// ```ruby
    /// yyerrok
    /// ```
    A0a57c26f,
    /// 使用箇所: ruby33 81 (command), ruby34 81 (command).
    ///
    /// ```ruby
    /// method_call = @builder.call_method(val[0], val[1], val[2],
    /// nil, [], nil)
    /// args, body = val[4]
    /// result = @builder.block(method_call, val[3], args, body, val[5])
    /// ```
    A0af1f95a,
    /// 使用箇所: ruby27 321 (primary), ruby30 330 (primary), ruby31 337 (primary), ruby32 339 (primary) and 2 more.
    ///
    /// ```ruby
    /// else_t, else_ = val[4]
    /// result = @builder.condition(val[0], val[1], val[2],
    /// else_,  else_t,
    /// val[3], val[5])
    /// ```
    A0bc894ba,
    /// 使用箇所: ruby27 502 (p_kwrest), ruby30 512 (p_kwrest), ruby31 522 (p_kwrest), ruby32 522 (p_kwrest) and 2 more.
    ///
    /// ```ruby
    /// result = [ @builder.match_rest(val[0], val[1]) ]
    /// ```
    A0c82eb92,
    /// 使用箇所: ruby34 346 (@11).
    ///
    /// ```ruby
    /// @context.in_def = false
    /// @context.in_class = false
    /// @context.cant_return = true
    /// local_push
    /// ```
    A0d431771,
    /// 使用箇所: ruby27 637 (f_bad_arg), ruby30 650 (f_bad_arg), ruby31 664 (f_bad_arg), ruby32 662 (f_bad_arg) and 2 more.
    ///
    /// ```ruby
    /// diagnostic :error, :argument_const, nil, val[0]
    /// ```
    A0edb9197,
    /// 使用箇所: ruby27 40 (command_asgn), ruby27 41 (command_asgn), ruby27 43 (command_asgn), ruby27 204 (arg) and 32 more.
    ///
    /// ```ruby
    /// result = @builder.op_assign(
    /// @builder.call_method(
    /// val[0], val[1], val[2]),
    /// val[3], val[4])
    /// ```
    A11286b4d,
    /// 使用箇所: ruby27 4 (top_stmts), ruby27 13 (stmts), ruby27 262 (opt_call_args), ruby27 276 (opt_block_arg) and 104 more.
    ///
    /// ```ruby
    /// result = []
    /// ```
    A1142a778,
    /// 使用箇所: ruby27 408 (lambda_body), ruby27 410 (lambda_body), ruby30 416 (lambda_body), ruby30 418 (lambda_body).
    ///
    /// ```ruby
    /// result = [ val[0], val[2], val[3] ]
    /// @context.in_lambda = val[1].in_lambda
    /// ```
    A11e3700b,
    /// 使用箇所: ruby27 527 (p_var_ref), ruby30 538 (p_var_ref), ruby31 548 (p_var_ref), ruby32 548 (p_var_ref) and 2 more.
    ///
    /// ```ruby
    /// name = val[1][0]
    /// lvar = @builder.accessible(@builder.ident(val[1]))
    /// unless static_env.declared?(name)
    /// diagnostic :error, :undefined_lvar, { :name => name }, val[1]
    /// end
    /// result = @builder.pin(val[0], lvar)
    /// ```
    A138e5de5,
    /// 使用箇所: ruby27 570 (string_content), ruby30 581 (string_content), ruby31 593 (string_content), ruby32 593 (string_content) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.string_internal(val[0])
    /// ```
    A13977d88,
    /// 使用箇所: ruby27 334 (@15), ruby27 337 (@17).
    ///
    /// ```ruby
    /// local_push
    /// result = context.dup
    /// @context.in_def = true
    /// @current_arg_stack.push(nil)
    /// ```
    A13dfe951,
    /// 使用箇所: ruby27 18 (stmt_or_begin), ruby30 18 (stmt_or_begin), ruby31 18 (stmt_or_begin), ruby32 18 (stmt_or_begin) and 2 more.
    ///
    /// ```ruby
    /// diagnostic :error, :begin_in_method, nil, val[0]
    /// ```
    A1405bbe5,
    /// 使用箇所: ruby27 486 (p_args_tail).
    ///
    /// ```ruby
    /// match_rest = @builder.match_rest(val[0])
    /// result = [ match_rest, *val[2] ]
    /// ```
    A15272a42,
    /// 使用箇所: ruby27 404 (lambda).
    ///
    /// ```ruby
    /// args = @max_numparam_stack.has_numparams? ? @builder.numargs(@max_numparam_stack.top) : val[1]
    /// result = [ args, val[3] ]
    /// @max_numparam_stack.pop
    /// @static_env.unextend
    /// @lexer.cmdarg.pop
    /// ```
    A1558c0ae,
    /// 使用箇所: ruby27 608 (superclass), ruby27 715 (none), ruby30 619 (superclass), ruby30 728 (none) and 8 more.
    ///
    /// ```ruby
    /// result = nil
    /// ```
    A157c9d67,
    /// 使用箇所: ruby33 459 (p_case_body), ruby34 459 (p_case_body).
    ///
    /// ```ruby
    /// result = [ @builder.in_pattern(val[0], *val[4], val[5], val[7]),
    /// *val[8] ]
    /// ```
    A16eb4208,
    /// 使用箇所: ruby27 212 (arg), ruby27 508 (p_value), ruby30 218 (arg), ruby30 520 (p_value) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.range_inclusive(val[0], val[1], nil)
    /// ```
    A17ef1889,
    /// 使用箇所: ruby27 310 (primary), ruby30 320 (primary), ruby31 326 (primary), ruby32 328 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:yield, val[0], val[1], [], val[2])
    /// ```
    A18e81f7e,
    /// 使用箇所: ruby27 476 (p_args), ruby30 487 (p_args), ruby31 497 (p_args).
    ///
    /// ```ruby
    /// match_rest = @builder.match_rest(val[1], val[2])
    /// result = [ *val[0], match_rest ]
    /// ```
    A19e23ea1,
    /// 使用箇所: ruby27 440 (p_case_body), ruby30 448 (p_case_body), ruby31 457 (p_case_body), ruby32 459 (p_case_body).
    ///
    /// ```ruby
    /// result = [ @builder.in_pattern(val[0], *val[2], val[3], val[5]),
    /// *val[6] ]
    /// ```
    A1a2a275c,
    /// 使用箇所: ruby27 485 (p_args_tail).
    ///
    /// ```ruby
    /// match_rest = @builder.match_rest(val[0])
    /// result = [ match_rest ]
    /// ```
    A1ce7b2cc,
    /// 使用箇所: ruby27 424 (method_call), ruby30 432 (method_call), ruby31 441 (method_call), ruby32 443 (method_call) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:zsuper, val[0])
    /// ```
    A1d1cf16b,
    /// 使用箇所: ruby27 347 (k_return), ruby30 353 (k_return), ruby31 360 (k_return), ruby32 362 (k_return) and 1 more.
    ///
    /// ```ruby
    /// if @context.in_class && !@context.in_def && !(context.in_block || context.in_lambda)
    /// diagnostic :error, :invalid_return, nil, val[0]
    /// end
    /// ```
    A203b4421,
    /// 使用箇所: ruby27 400 (bvar), ruby30 408 (bvar), ruby31 417 (bvar), ruby32 419 (bvar) and 2 more.
    ///
    /// ```ruby
    /// @static_env.declare val[0][0]
    /// result = @builder.shadowarg(val[0])
    /// ```
    A21a2f7f9,
    /// 使用箇所: ruby33 715 (assoc), ruby34 715 (assoc).
    ///
    /// ```ruby
    /// if !@static_env.declared_anonymous_kwrestarg?
    /// diagnostic :error, :no_anonymous_kwrestarg, nil, val[0]
    /// end
    /// if @context.in_dynamic_block? && context.in_def &&
    /// @static_env.declared_anonymous_kwrestarg_in_current_scope? && @static_env.parent_has_anonymous_kwrestarg?
    /// diagnostic :error, :ambiguous_anonymous_kwrestarg, nil, val[0]
    /// end
    /// result = @builder.forwarded_kwrestarg(val[0])
    /// ```
    A2253a24d,
    /// 使用箇所: ruby27 685 (assoc), ruby30 698 (assoc), ruby31 714 (assoc), ruby32 712 (assoc) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.pair_quoted(val[0], val[1], val[2], val[3])
    /// ```
    A22720faf,
    /// 使用箇所: ruby27 415 (block_call), ruby30 423 (block_call), ruby31 432 (block_call), ruby32 434 (block_call) and 2 more.
    ///
    /// ```ruby
    /// lparen_t, args, rparen_t = val[3]
    /// method_call = @builder.call_method(val[0], val[1], val[2],
    /// lparen_t, args, rparen_t)
    /// begin_t, args, body, end_t = val[4]
    /// result      = @builder.block(method_call,
    /// begin_t, args, body, end_t)
    /// ```
    A22b49fd6,
    /// 使用箇所: ruby27 30 (stmt), ruby30 30 (stmt), ruby31 30 (stmt), ruby32 30 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.postexe(val[0], val[1], val[2], val[3])
    /// ```
    A23524b49,
    /// 使用箇所: ruby27 659 (f_no_kwarg), ruby30 672 (f_no_kwarg), ruby31 686 (f_no_kwarg).
    ///
    /// ```ruby
    /// result = [ @builder.kwnilarg(val[0], val[1]) ]
    /// ```
    A23f7ac27,
    /// 使用箇所: ruby31 675 (f_label), ruby32 673 (f_label), ruby33 674 (f_label), ruby34 674 (f_label).
    ///
    /// ```ruby
    /// check_kwarg_name(val[0])
    /// @static_env.declare val[0][0]
    /// @max_numparam_stack.has_ordinary_params!
    /// @current_arg_stack.set(val[0][0])
    /// @context.in_argdef = false
    /// result = val[0]
    /// ```
    A24624c28,
    /// 使用箇所: ruby27 470 (p_expr_basic), ruby30 481 (p_expr_basic), ruby31 491 (p_expr_basic), ruby32 493 (p_expr_basic) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.hash_pattern(val[0], [], val[1])
    /// ```
    A2542d91f,
    /// 使用箇所: ruby27 384 (block_param), ruby30 392 (block_param), ruby31 401 (block_param), ruby32 403 (block_param) and 2 more.
    ///
    /// ```ruby
    /// if val[1].empty? && val[0].size == 1
    /// result = [@builder.procarg0(val[0][0])]
    /// else
    /// result = val[0].concat(val[1])
    /// end
    /// ```
    A26ba6e5a,
    /// 使用箇所: ruby27 268 (call_args), ruby27 371 (block_args_tail), ruby27 372 (block_args_tail), ruby27 373 (block_args_tail) and 28 more.
    ///
    /// ```ruby
    /// result = val[0].concat(val[1])
    /// ```
    A2747f67b,
    /// 使用箇所: ruby27 260 (opt_paren_args), ruby30 270 (opt_paren_args), ruby31 275 (opt_paren_args), ruby32 275 (opt_paren_args) and 2 more.
    ///
    /// ```ruby
    /// result = [ nil, [], nil ]
    /// ```
    A2752b003,
    /// 使用箇所: ruby27 699 (call_op), ruby30 712 (call_op), ruby31 728 (call_op), ruby32 725 (call_op) and 2 more.
    ///
    /// ```ruby
    /// result = [:dot, val[0][1]]
    /// ```
    A27e22b15,
    /// 使用箇所: ruby27 555 (symbols), ruby27 559 (qsymbols), ruby30 566 (symbols), ruby30 570 (qsymbols) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.symbols_compose(val[0], val[1], val[2])
    /// ```
    A2827caca,
    /// 使用箇所: ruby27 322 (primary), ruby30 331 (primary), ruby31 338 (primary), ruby32 340 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.loop(:while, val[0], *val[1], val[2], val[3])
    /// ```
    A299bd0ad,
    /// 使用箇所: ruby31 359 (k_def), ruby32 361 (k_def), ruby33 359 (k_def), ruby34 359 (k_def).
    ///
    /// ```ruby
    /// result = val[0]
    /// @context.in_argdef = true
    /// ```
    A29fa6e15,
    /// 使用箇所: ruby27 325 (primary), ruby30 334 (primary), ruby31 341 (primary), ruby32 343 (primary) and 2 more.
    ///
    /// ```ruby
    /// *when_bodies, (else_t, else_body) = *val[2]
    /// result = @builder.case(val[0], nil,
    /// when_bodies, else_t, else_body,
    /// val[3])
    /// ```
    A2ad94566,
    /// 使用箇所: ruby27 364 (f_margs), ruby30 370 (f_margs), ruby31 377 (f_margs), ruby32 379 (f_margs) and 2 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// push(val[2])
    /// ```
    A2c0631bd,
    /// 使用箇所: ruby34 349 (primary).
    ///
    /// ```ruby
    /// k_mod, ctx = val[0]
    /// if @context.in_def
    /// diagnostic :error, :module_in_def, nil, k_mod
    /// end
    /// result = @builder.def_module(k_mod, val[1],
    /// val[3], val[4])
    /// local_pop
    /// @context.in_class = ctx.in_class
    /// @context.cant_return = ctx.cant_return
    /// ```
    A2c79d925,
    /// 使用箇所: ruby27 258 (paren_args), ruby30 268 (paren_args), ruby31 273 (paren_args), ruby32 273 (paren_args) and 2 more.
    ///
    /// ```ruby
    /// unless @static_env.declared_forward_args?
    /// diagnostic :error, :unexpected_token, { :token => 'tBDOT3' } , val[3]
    /// end
    /// result = [val[0], [*val[1], @builder.forwarded_args(val[3])], val[4]]
    /// ```
    A2d1d5e63,
    /// 使用箇所: ruby27 326 (primary), ruby30 335 (primary), ruby31 342 (primary), ruby32 344 (primary) and 2 more.
    ///
    /// ```ruby
    /// *in_bodies, (else_t, else_body) = *val[3]
    /// result = @builder.case_match(val[0], val[1],
    /// in_bodies, else_t, else_body,
    /// val[4])
    /// ```
    A2dc3cb3c,
    /// 使用箇所: ruby27 208 (arg), ruby30 214 (arg), ruby31 218 (arg), ruby32 218 (arg) and 2 more.
    ///
    /// ```ruby
    /// const  = @builder.const_op_assignable(
    /// @builder.const_global(val[0], val[1]))
    /// result = @builder.op_assign(const, val[2], val[3])
    /// ```
    A2e05b122,
    /// 使用箇所: ruby27 99 (mlhs_node), ruby27 108 (lhs), ruby30 105 (mlhs_node), ruby30 114 (lhs) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.index_asgn(val[0], val[1], val[2], val[3])
    /// ```
    A2e3b88ae,
    /// 使用箇所: ruby27 501 (p_kw_label), ruby30 511 (p_kw_label), ruby31 521 (p_kw_label), ruby32 521 (p_kw_label) and 2 more.
    ///
    /// ```ruby
    /// result = [:quoted, [val[0], val[1], val[2]]]
    /// ```
    A2e601b72,
    /// 使用箇所: ruby27 83 (mlhs_basic), ruby30 89 (mlhs_basic), ruby31 93 (mlhs_basic), ruby32 93 (mlhs_basic) and 2 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// push(@builder.splat(val[1], val[2]))
    /// ```
    A2e8e96f1,
    /// 使用箇所: ruby27 369 (f_rest_marg), ruby30 375 (f_rest_marg), ruby31 382 (f_rest_marg), ruby32 384 (f_rest_marg) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.restarg(val[0])
    /// ```
    A2f0bd7c9,
    /// 使用箇所: ruby27 670 (f_rest_arg), ruby30 683 (f_rest_arg), ruby31 697 (f_rest_arg), ruby32 695 (f_rest_arg) and 2 more.
    ///
    /// ```ruby
    /// @static_env.declare val[1][0]
    /// result = [ @builder.restarg(val[0], val[1]) ]
    /// ```
    A2faf9fc6,
    /// 使用箇所: ruby27 3 (top_compstmt), ruby27 12 (compstmt), ruby30 3 (top_compstmt), ruby30 12 (compstmt) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.compstmt(val[0])
    /// ```
    A303c0362,
    /// 使用箇所: ruby27 32 (stmt), ruby27 35 (stmt), ruby30 32 (stmt), ruby30 35 (stmt) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.multi_assign(val[0], val[1], val[2])
    /// ```
    A3073dff7,
    /// 使用箇所: ruby27 97 (mlhs_node), ruby27 98 (mlhs_node), ruby27 105 (mlhs_node), ruby27 106 (lhs) and 44 more.
    ///
    /// ```ruby
    /// result = @builder.assignable(val[0])
    /// ```
    A3119d3f8,
    /// 使用箇所: ruby30 56 (expr).
    ///
    /// ```ruby
    /// @pattern_variables.pop
    /// @context.in_kwarg = val[2]
    /// result = @builder.match_pattern_p(val[0], val[1], val[3])
    /// ```
    A31a6246c,
    /// 使用箇所: ruby31 290 (block_arg), ruby32 290 (block_arg).
    ///
    /// ```ruby
    /// if !@static_env.declared_anonymous_blockarg?
    /// diagnostic :error, :no_anonymous_blockarg, nil, val[0]
    /// end
    /// result = @builder.block_pass(val[0], nil)
    /// ```
    A32f7fec2,
    /// 使用箇所: ruby27 454 (p_alt), ruby30 463 (p_alt), ruby31 472 (p_alt), ruby32 474 (p_alt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.match_alt(val[0], val[1], val[2])
    /// ```
    A3387c285,
    /// 使用箇所: ruby27 37 (command_asgn), ruby27 201 (arg), ruby30 37 (command_asgn), ruby30 207 (arg) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.assign(val[0], val[1], val[2])
    /// ```
    A339b1bf4,
    /// 使用箇所: ruby30 469 (p_expr_basic), ruby30 473 (p_expr_basic), ruby31 479 (p_expr_basic), ruby31 483 (p_expr_basic) and 6 more.
    ///
    /// ```ruby
    /// @pattern_hash_keys.pop
    /// pattern = @builder.find_pattern(nil, val[2], nil)
    /// result = @builder.const_pattern(val[0], val[1], pattern, val[3])
    /// ```
    A33e0d886,
    /// 使用箇所: ruby27 89 (mlhs_basic), ruby30 95 (mlhs_basic), ruby31 99 (mlhs_basic), ruby32 99 (mlhs_basic) and 2 more.
    ///
    /// ```ruby
    /// result = [ @builder.splat(val[0]) ]
    /// ```
    A34be6a6c,
    /// 使用箇所: ruby27 53 (@3), ruby30 53 (@3), ruby30 55 (@4).
    ///
    /// ```ruby
    /// @lexer.state = :expr_beg
    /// @lexer.command_start = false
    /// @pattern_variables.push
    /// result = @context.in_kwarg
    /// @context.in_kwarg = true
    /// ```
    A356620a4,
    /// 使用箇所: ruby27 119 (cpath), ruby27 304 (primary), ruby27 529 (p_const), ruby30 125 (cpath) and 14 more.
    ///
    /// ```ruby
    /// result = @builder.const_fetch(val[0], val[1], val[2])
    /// ```
    A367af191,
    /// 使用箇所: ruby27 587 (simple_numeric), ruby30 598 (simple_numeric), ruby31 610 (simple_numeric), ruby32 610 (simple_numeric) and 2 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_end
    /// result = @builder.complex(val[0])
    /// ```
    A36f003dd,
    /// 使用箇所: ruby33 265 (begin_defined), ruby34 265 (begin_defined).
    ///
    /// ```ruby
    /// result = @context.dup
    /// ```
    A3717d7b6,
    /// 使用箇所: ruby27 444 (p_top_expr), ruby30 452 (p_top_expr), ruby31 461 (p_top_expr), ruby32 463 (p_top_expr) and 2 more.
    ///
    /// ```ruby
    /// result = [ val[0], @builder.if_guard(val[1], val[2]) ]
    /// ```
    A3736f0d1,
    /// 使用箇所: ruby31 251 (@9), ruby31 328 (@14), ruby32 251 (@9), ruby32 330 (@14).
    ///
    /// ```ruby
    /// @context.in_defined = true
    /// ```
    A37eb4c5a,
    /// 使用箇所: ruby30 251 (arg).
    ///
    /// ```ruby
    /// def_t, recv, dot_t, (name_t, ctx) = val[0]
    /// endless_method_name(name_t)
    /// result = @builder.def_endless_singleton(def_t, recv, dot_t, name_t,
    /// val[1], val[2], val[3])
    /// local_pop
    /// @context.in_def = ctx.in_def
    /// @current_arg_stack.pop
    /// ```
    A387abb07,
    /// 使用箇所: ruby27 84 (mlhs_basic), ruby30 90 (mlhs_basic), ruby31 94 (mlhs_basic), ruby32 94 (mlhs_basic) and 2 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// push(@builder.splat(val[1], val[2])).
    /// concat(val[4])
    /// ```
    A3881d925,
    /// 使用箇所: ruby27 662 (f_opt), ruby27 663 (f_block_opt), ruby30 675 (f_opt), ruby30 676 (f_block_opt).
    ///
    /// ```ruby
    /// @current_arg_stack.set(0)
    /// result = @builder.optarg(val[0], val[1], val[2])
    /// ```
    A3962bcd1,
    /// 使用箇所: ruby31 74 (cmd_brace_block), ruby31 429 (do_block), ruby31 444 (brace_block), ruby31 446 (brace_block) and 12 more.
    ///
    /// ```ruby
    /// @context.in_block = val[1].in_block
    /// result = [ val[0], *val[2], val[3] ]
    /// ```
    A3a13bf77,
    /// 使用箇所: ruby31 702 (f_block_arg), ruby32 700 (f_block_arg), ruby33 701 (f_block_arg), ruby34 701 (f_block_arg).
    ///
    /// ```ruby
    /// @static_env.declare_anonymous_blockarg
    /// result = @builder.blockarg(val[0], nil)
    /// ```
    A3bcec143,
    /// 使用箇所: ruby31 713 (assoc), ruby32 711 (assoc), ruby33 712 (assoc), ruby34 712 (assoc).
    ///
    /// ```ruby
    /// result = @builder.pair_label(val[0])
    /// ```
    A3c92c687,
    /// 使用箇所: ruby27 85 (mlhs_basic), ruby30 91 (mlhs_basic), ruby31 95 (mlhs_basic), ruby32 95 (mlhs_basic) and 2 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// push(@builder.splat(val[1]))
    /// ```
    A3ec5887f,
    /// 使用箇所: ruby27 650 (f_kw), ruby30 663 (f_kw).
    ///
    /// ```ruby
    /// @current_arg_stack.set(nil)
    /// result = @builder.kwarg(val[0])
    /// ```
    A3edbb047,
    /// 使用箇所: ruby27 241 (arg), ruby30 247 (arg).
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:defined?, val[0], nil, [ val[2] ], nil)
    /// ```
    A3f241b5e,
    /// 使用箇所: ruby27 598 (keyword_variable), ruby30 609 (keyword_variable), ruby31 624 (keyword_variable), ruby32 622 (keyword_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.__LINE__(val[0])
    /// ```
    A3fef0dcd,
    /// 使用箇所: ruby30 54 (expr).
    ///
    /// ```ruby
    /// @pattern_variables.pop
    /// @context.in_kwarg = val[2]
    /// result = @builder.match_pattern(val[0], val[1], val[3])
    /// ```
    A40375c56,
    /// 使用箇所: ruby27 536 (exc_var), ruby27 538 (opt_ensure), ruby30 62 (defn_head), ruby30 547 (exc_var) and 13 more.
    ///
    /// ```ruby
    /// result = [ val[0], val[1] ]
    /// ```
    A4133a42a,
    /// 使用箇所: ruby27 638 (f_bad_arg), ruby30 651 (f_bad_arg), ruby31 665 (f_bad_arg), ruby32 663 (f_bad_arg) and 2 more.
    ///
    /// ```ruby
    /// diagnostic :error, :argument_ivar, nil, val[0]
    /// ```
    A42b4c309,
    /// 使用箇所: ruby27 642 (f_norm_arg), ruby30 655 (f_norm_arg), ruby31 669 (f_norm_arg), ruby32 667 (f_norm_arg) and 2 more.
    ///
    /// ```ruby
    /// @static_env.declare val[0][0]
    /// @max_numparam_stack.has_ordinary_params!
    /// result = val[0]
    /// ```
    A45782483,
    /// 使用箇所: ruby27 324 (primary), ruby30 333 (primary), ruby31 340 (primary), ruby32 342 (primary) and 2 more.
    ///
    /// ```ruby
    /// *when_bodies, (else_t, else_body) = *val[3]
    /// result = @builder.case(val[0], val[1],
    /// when_bodies, else_t, else_body,
    /// val[4])
    /// ```
    A45a75655,
    /// 使用箇所: ruby27 34 (stmt), ruby30 34 (stmt), ruby31 34 (stmt), ruby32 34 (stmt) and 2 more.
    ///
    /// ```ruby
    /// rescue_body = @builder.rescue_body(val[3],
    /// nil, nil, nil,
    /// nil, val[4])
    /// begin_body = @builder.begin_body(val[2], [ rescue_body ])
    /// result = @builder.multi_assign(val[0], val[1], begin_body)
    /// ```
    A46561a17,
    /// 使用箇所: ruby32 686 (f_kwrest), ruby33 687 (f_kwrest), ruby34 687 (f_kwrest).
    ///
    /// ```ruby
    /// @static_env.declare_anonymous_kwrestarg
    /// result = [ @builder.kwrestarg(val[0]) ]
    /// ```
    A480387b1,
    /// 使用箇所: ruby27 607 (superclass), ruby30 618 (superclass), ruby31 633 (superclass), ruby32 631 (superclass) and 2 more.
    ///
    /// ```ruby
    /// result = [ val[0], val[2] ]
    /// ```
    A485752ad,
    /// 使用箇所: ruby27 346 (k_def), ruby27 474 (p_args), ruby27 491 (p_kwargs), ruby27 492 (p_kwargs) and 40 more.
    ///
    /// ```ruby
    /// result = val[0]
    /// ```
    A48aefcf2,
    /// 使用箇所: ruby27 447 (p_top_expr_body), ruby30 455 (p_top_expr_body), ruby31 464 (p_top_expr_body), ruby32 466 (p_top_expr_body) and 2 more.
    ///
    /// ```ruby
    /// item = @builder.match_with_trailing_comma(val[0], val[1])
    /// result = @builder.array_pattern(nil, [ item ], nil)
    /// ```
    A48cf2658,
    /// 使用箇所: ruby27 73 (command), ruby30 79 (command), ruby31 83 (command), ruby32 83 (command) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:yield, val[0],
    /// nil, val[1], nil)
    /// ```
    A49880833,
    /// 使用箇所: ruby27 273 (command_args), ruby30 283 (command_args), ruby31 288 (command_args), ruby32 288 (command_args) and 2 more.
    ///
    /// ```ruby
    /// last_token = @last_token[0]
    /// lookahead = last_token == :tLBRACE_ARG
    /// if lookahead
    /// top = @lexer.cmdarg.pop
    /// @lexer.cmdarg.pop
    /// @lexer.cmdarg.push(top)
    /// else
    /// @lexer.cmdarg.pop
    /// end
    /// result = val[1]
    /// ```
    A4b3fea33,
    /// 使用箇所: ruby27 24 (stmt), ruby30 24 (stmt), ruby31 24 (stmt), ruby32 24 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.undef_method(val[0], val[1])
    /// ```
    A4bbff142,
    /// 使用箇所: ruby27 317 (primary), ruby30 327 (primary), ruby31 334 (primary), ruby32 336 (primary) and 2 more.
    ///
    /// ```ruby
    /// begin_t, args, body, end_t = val[1]
    /// result      = @builder.block(val[0],
    /// begin_t, args, body, end_t)
    /// ```
    A4c29d9cc,
    /// 使用箇所: ruby27 406 (f_larglist), ruby30 414 (f_larglist).
    ///
    /// ```ruby
    /// if val[0].any?
    /// @max_numparam_stack.has_ordinary_params!
    /// end
    /// result = @builder.args(nil, val[0], nil)
    /// ```
    A4ecf11c6,
    /// 使用箇所: ruby27 526 (p_variable), ruby30 537 (p_variable), ruby31 547 (p_variable), ruby32 547 (p_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.assignable(@builder.match_var(val[0]))
    /// ```
    A4f51b6d8,
    /// 使用箇所: ruby31 422 (f_larglist), ruby32 424 (f_larglist), ruby33 422 (f_larglist), ruby34 422 (f_larglist).
    ///
    /// ```ruby
    /// @context.in_argdef = false
    /// @max_numparam_stack.has_ordinary_params!
    /// result = @builder.args(val[0], val[1].concat(val[2]), val[3])
    /// ```
    A4fb1fd6d,
    /// 使用箇所: ruby27 125 (fitem), ruby30 131 (fitem), ruby31 135 (fitem), ruby32 135 (fitem) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.symbol_internal(val[0])
    /// ```
    A4ffc260f,
    /// 使用箇所: ruby27 379 (block_param), ruby27 381 (block_param), ruby27 385 (block_param), ruby27 388 (block_param) and 56 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// concat(val[2]).
    /// concat(val[3])
    /// ```
    A50bb5ee5,
    /// 使用箇所: ruby31 645 (args_tail), ruby32 643 (args_tail), ruby33 644 (args_tail), ruby34 644 (args_tail).
    ///
    /// ```ruby
    /// @static_env.declare_forward_args
    /// result = [ @builder.forward_arg(val[0]) ]
    /// ```
    A50d6c102,
    /// 使用箇所: ruby27 405 (f_larglist), ruby30 413 (f_larglist).
    ///
    /// ```ruby
    /// @max_numparam_stack.has_ordinary_params!
    /// result = @builder.args(val[0], val[1].concat(val[2]), val[3])
    /// ```
    A51d05898,
    /// 使用箇所: ruby31 46 (command_asgn), ruby31 256 (arg), ruby32 46 (command_asgn), ruby32 256 (arg) and 4 more.
    ///
    /// ```ruby
    /// def_t, recv, dot_t, (name_t, ctx) = val[0]
    /// endless_method_name(name_t)
    /// result = @builder.def_endless_singleton(def_t, recv, dot_t, name_t,
    /// val[1], val[2], val[3])
    /// local_pop
    /// @current_arg_stack.pop
    /// @context.in_def = ctx.in_def
    /// ```
    A51e7c90c,
    /// 使用箇所: ruby27 499 (p_kw), ruby30 509 (p_kw), ruby31 519 (p_kw), ruby32 519 (p_kw) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.match_label(*val[0])
    /// ```
    A51f8df8e,
    /// 使用箇所: ruby27 417 (method_call), ruby30 425 (method_call), ruby31 434 (method_call), ruby32 436 (method_call) and 2 more.
    ///
    /// ```ruby
    /// lparen_t, args, rparen_t = val[1]
    /// result = @builder.call_method(nil, nil, val[0],
    /// lparen_t, args, rparen_t)
    /// ```
    A527c0661,
    /// 使用箇所: ruby27 472 (p_expr_basic), ruby30 483 (p_expr_basic), ruby31 493 (p_expr_basic), ruby32 495 (p_expr_basic) and 2 more.
    ///
    /// ```ruby
    /// @pattern_hash_keys.pop
    /// result = @builder.begin(val[0], val[2], val[3])
    /// ```
    A5532b1ff,
    /// 使用箇所: ruby27 504 (p_kwnorest), ruby30 514 (p_kwnorest), ruby31 524 (p_kwnorest).
    ///
    /// ```ruby
    /// result = [ @builder.match_nil_pattern(val[0], val[1]) ]
    /// ```
    A56118dc0,
    /// 使用箇所: ruby27 38 (command_asgn), ruby27 202 (arg), ruby27 209 (arg), ruby30 38 (command_asgn) and 14 more.
    ///
    /// ```ruby
    /// result = @builder.op_assign(val[0], val[1], val[2])
    /// ```
    A56d07ccc,
    /// 使用箇所: ruby27 67 (command), ruby30 73 (command), ruby31 77 (command), ruby32 77 (command) and 2 more.
    ///
    /// ```ruby
    /// method_call = @builder.call_method(nil, nil, val[0],
    /// nil, val[1], nil)
    /// begin_t, args, body, end_t = val[2]
    /// result      = @builder.block(method_call,
    /// begin_t, args, body, end_t)
    /// ```
    A56df17a3,
    /// 使用箇所: ruby27 597 (keyword_variable), ruby30 608 (keyword_variable), ruby31 623 (keyword_variable), ruby32 621 (keyword_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.__FILE__(val[0])
    /// ```
    A57dea737,
    /// 使用箇所: ruby31 636 (f_opt_paren_args), ruby32 634 (f_opt_paren_args), ruby33 635 (f_opt_paren_args), ruby34 635 (f_opt_paren_args).
    ///
    /// ```ruby
    /// @context.in_argdef = false
    /// result = @builder.args(nil, [], nil)
    /// ```
    A58621c89,
    /// 使用箇所: ruby27 19 (@2), ruby27 128 (@6), ruby27 336 (@16), ruby30 19 (@2) and 10 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_fname
    /// ```
    A589c228e,
    /// 使用箇所: ruby27 214 (arg), ruby27 513 (p_value), ruby30 220 (arg), ruby30 525 (p_value) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.range_inclusive(nil, val[0], val[1])
    /// ```
    A59476aca,
    /// 使用箇所: ruby27 26 (stmt), ruby30 26 (stmt), ruby31 26 (stmt), ruby32 26 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.condition_mod(nil, val[0],
    /// val[1], val[2])
    /// ```
    A59f79666,
    /// 使用箇所: ruby27 588 (user_variable), ruby30 599 (user_variable), ruby31 614 (user_variable), ruby32 614 (user_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.ident(val[0])
    /// ```
    A5cc384f8,
    /// 使用箇所: ruby27 211 (arg), ruby27 507 (p_value), ruby30 217 (arg), ruby30 519 (p_value) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.range_exclusive(val[0], val[1], val[2])
    /// ```
    A5cfd954b,
    /// 使用箇所: ruby27 342 (primary), ruby30 348 (primary), ruby31 355 (primary), ruby32 357 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:retry, val[0])
    /// ```
    A5d7f07af,
    /// 使用箇所: ruby27 465 (@30), ruby27 471 (@32), ruby30 482 (@29), ruby31 492 (@32) and 5 more.
    ///
    /// ```ruby
    /// @pattern_hash_keys.push
    /// ```
    A5da315f8,
    /// 使用箇所: ruby27 28 (stmt), ruby30 28 (stmt), ruby31 28 (stmt), ruby32 28 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.loop_mod(:until, val[0], val[1], val[2])
    /// ```
    A5dc3c211,
    /// 使用箇所: ruby27 297 (@8), ruby27 403 (@19), ruby27 433 (@27), ruby30 307 (@10) and 14 more.
    ///
    /// ```ruby
    /// @lexer.cmdarg.push(false)
    /// ```
    A5dc3e21b,
    /// 使用箇所: ruby27 74 (command), ruby30 80 (command), ruby31 84 (command), ruby32 84 (command) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:return, val[0],
    /// nil, val[1], nil)
    /// ```
    A6007f1a1,
    /// 使用箇所: ruby33 59 (expr), ruby34 59 (expr).
    ///
    /// ```ruby
    /// @pattern_variables.pop
    /// @pattern_hash_keys.pop
    /// @context.in_kwarg = val[2]
    /// result = @builder.match_pattern_p(val[0], val[1], val[5])
    /// ```
    A617d1edf,
    /// 使用箇所: ruby33 299 (arg_splat), ruby34 299 (arg_splat).
    ///
    /// ```ruby
    /// if !@static_env.declared_anonymous_restarg?
    /// diagnostic :error, :no_anonymous_restarg, nil, val[0]
    /// end
    /// if @context.in_dynamic_block? && context.in_def &&
    /// @static_env.declared_anonymous_restarg_in_current_scope? && @static_env.parent_has_anonymous_restarg?
    /// diagnostic :error, :ambiguous_anonymous_restarg, nil, val[0]
    /// end
    /// result = [ @builder.forwarded_restarg(val[0]) ]
    /// ```
    A636f6044,
    /// 使用箇所: ruby27 25 (stmt), ruby30 25 (stmt), ruby31 25 (stmt), ruby32 25 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.condition_mod(val[0], nil,
    /// val[1], val[2])
    /// ```
    A644653a1,
    /// 使用箇所: ruby32 684 (f_no_kwarg), ruby33 685 (f_no_kwarg), ruby34 685 (f_no_kwarg).
    ///
    /// ```ruby
    /// result = [ @builder.kwnilarg(val[0][0], val[0][1]) ]
    /// ```
    A648fe6bd,
    /// 使用箇所: ruby27 313 (primary), ruby30 323 (primary), ruby31 330 (primary), ruby32 332 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.not_op(val[0], val[1], val[2], val[3])
    /// ```
    A64f508c4,
    /// 使用箇所: ruby27 296 (primary), ruby30 306 (primary), ruby31 312 (primary), ruby32 314 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.call_method(nil, nil, val[0])
    /// ```
    A6529c845,
    /// 使用箇所: ruby27 610 (f_arglist), ruby30 623 (f_paren_args).
    ///
    /// ```ruby
    /// args = [ *val[1], @builder.forward_arg(val[3]) ]
    /// result = @builder.args(val[0], args, val[4])
    /// @static_env.declare_forward_args
    /// ```
    A661e9e9c,
    /// 使用箇所: ruby27 599 (keyword_variable), ruby30 610 (keyword_variable), ruby31 625 (keyword_variable), ruby32 623 (keyword_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.__ENCODING__(val[0])
    /// ```
    A66259dda,
    /// 使用箇所: ruby27 596 (keyword_variable), ruby30 607 (keyword_variable), ruby31 622 (keyword_variable), ruby32 620 (keyword_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.false(val[0])
    /// ```
    A66b4bdeb,
    /// 使用箇所: ruby30 61 (def_name), ruby31 65 (def_name), ruby32 65 (def_name), ruby33 64 (def_name).
    ///
    /// ```ruby
    /// local_push
    /// @current_arg_stack.push(nil)
    /// result = [ val[0], @context.dup ]
    /// @context.in_def = true
    /// ```
    A66e3fece,
    /// 使用箇所: ruby27 302 (primary), ruby30 312 (primary), ruby31 318 (primary), ruby32 320 (primary).
    ///
    /// ```ruby
    /// result = @builder.begin(val[0], nil, val[3])
    /// ```
    A678b56e6,
    /// 使用箇所: ruby27 365 (f_margs), ruby30 371 (f_margs), ruby31 378 (f_margs), ruby32 380 (f_margs) and 2 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// push(val[2]).
    /// concat(val[4])
    /// ```
    A67976ae6,
    /// 使用箇所: ruby27 242 (arg), ruby30 248 (arg), ruby31 253 (arg), ruby32 253 (arg) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.ternary(val[0], val[1],
    /// val[2], val[4], val[5])
    /// ```
    A67d438c7,
    /// 使用箇所: ruby27 439 (@29).
    ///
    /// ```ruby
    /// @pattern_hash_keys.pop
    /// @pattern_variables.pop
    /// @context.in_kwarg = val[1]
    /// ```
    A68142ac8,
    /// 使用箇所: ruby27 308 (primary), ruby30 318 (primary), ruby31 324 (primary), ruby32 326 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:return, val[0])
    /// ```
    A690f7921,
    /// 使用箇所: ruby27 50 (expr), ruby27 240 (arg), ruby30 50 (expr), ruby30 246 (arg) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.logical_op(:or, val[0], val[1], val[2])
    /// ```
    A697f828e,
    /// 使用箇所: ruby27 649 (f_kw), ruby30 662 (f_kw).
    ///
    /// ```ruby
    /// @current_arg_stack.set(nil)
    /// result = @builder.kwoptarg(val[0], val[1])
    /// ```
    A69c4e18c,
    /// 使用箇所: ruby27 459 (p_expr_basic), ruby27 462 (p_expr_basic), ruby30 468 (p_expr_basic), ruby30 472 (p_expr_basic) and 8 more.
    ///
    /// ```ruby
    /// @pattern_hash_keys.pop
    /// pattern = @builder.array_pattern(nil, val[2], nil)
    /// result = @builder.const_pattern(val[0], val[1], pattern, val[3])
    /// ```
    A6a06943b,
    /// 使用箇所: ruby27 20 (stmt), ruby30 20 (stmt), ruby31 20 (stmt), ruby32 20 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.alias(val[0], val[1], val[3])
    /// ```
    A6a3141e7,
    /// 使用箇所: ruby27 542 (strings), ruby30 553 (strings), ruby31 565 (strings), ruby32 565 (strings) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.string_compose(nil, val[0], nil)
    /// ```
    A6a605d30,
    /// 使用箇所: ruby27 461 (p_expr_basic), ruby27 464 (p_expr_basic), ruby30 471 (p_expr_basic), ruby30 475 (p_expr_basic) and 8 more.
    ///
    /// ```ruby
    /// pattern = @builder.array_pattern(val[1], nil, val[2])
    /// result = @builder.const_pattern(val[0], val[1], pattern, val[2])
    /// ```
    A6b0b77af,
    /// 使用箇所: ruby27 613 (f_arglist), ruby30 627 (f_arglist).
    ///
    /// ```ruby
    /// @context.in_kwarg = val[0]
    /// result = @builder.args(nil, val[1], nil)
    /// ```
    A6b21adff,
    /// 使用箇所: ruby27 449 (p_top_expr_body), ruby30 458 (p_top_expr_body), ruby31 467 (p_top_expr_body), ruby32 469 (p_top_expr_body) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.array_pattern(nil, val[0], nil)
    /// ```
    A6c17ba60,
    /// 使用箇所: ruby33 297 (args), ruby34 297 (args).
    ///
    /// ```ruby
    /// result = val[0].concat(val[2])
    /// ```
    A6caa26d5,
    /// 使用箇所: ruby27 561 (qword_list), ruby30 572 (qword_list), ruby31 584 (qword_list), ruby32 584 (qword_list) and 2 more.
    ///
    /// ```ruby
    /// result = val[0] << @builder.string_internal(val[1])
    /// ```
    A6ce756fd,
    /// 使用箇所: ruby27 329 (primary), ruby30 338 (primary), ruby31 345 (primary), ruby32 347 (primary) and 1 more.
    ///
    /// ```ruby
    /// k_class, ctx = val[0]
    /// if @context.in_def
    /// diagnostic :error, :class_in_def, nil, k_class
    /// end
    /// lt_t, superclass = val[2]
    /// result = @builder.def_class(k_class, val[1],
    /// lt_t, superclass,
    /// val[4], val[5])
    /// local_pop
    /// @context.in_class = ctx.in_class
    /// ```
    A6d06cc4e,
    /// 使用箇所: ruby27 478 (p_args), ruby30 489 (p_args), ruby31 499 (p_args).
    ///
    /// ```ruby
    /// result = [ *val[0], @builder.match_rest(val[1]) ]
    /// ```
    A6f8c7025,
    /// 使用箇所: ruby27 272 (@7), ruby30 282 (@9), ruby31 287 (@10), ruby32 287 (@10) and 2 more.
    ///
    /// ```ruby
    /// last_token = @last_token[0]
    /// lookahead = last_token == :tLBRACK || last_token == :tLPAREN_ARG
    /// if lookahead
    /// top = @lexer.cmdarg.pop
    /// @lexer.cmdarg.push(true)
    /// @lexer.cmdarg.push(top)
    /// else
    /// @lexer.cmdarg.push(true)
    /// end
    /// ```
    A7143a6f0,
    /// 使用箇所: ruby27 425 (method_call), ruby30 433 (method_call), ruby31 442 (method_call), ruby32 444 (method_call) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.index(val[0], val[1], val[2], val[3])
    /// ```
    A7316e5e0,
    /// 使用箇所: ruby27 307 (primary), ruby30 317 (primary), ruby31 323 (primary), ruby32 325 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.associate(val[0], val[1], val[2])
    /// ```
    A733530b7,
    /// 使用箇所: ruby27 274 (block_arg), ruby30 284 (block_arg), ruby31 289 (block_arg), ruby32 289 (block_arg) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.block_pass(val[0], val[1])
    /// ```
    A7462e75e,
    /// 使用箇所: ruby33 251 (arg), ruby34 251 (arg).
    ///
    /// ```ruby
    /// @context.in_defined = val[2].in_defined
    /// result = @builder.keyword_cmd(:defined?, val[0], nil, [ val[3] ], nil)
    /// ```
    A74d326ff,
    /// 使用箇所: ruby27 503 (p_kwrest), ruby30 513 (p_kwrest), ruby31 523 (p_kwrest), ruby32 523 (p_kwrest) and 2 more.
    ///
    /// ```ruby
    /// result = [ @builder.match_rest(val[0], nil) ]
    /// ```
    A764d8649,
    /// 使用箇所: ruby27 39 (command_asgn), ruby27 203 (arg), ruby30 39 (command_asgn), ruby30 209 (arg) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.op_assign(
    /// @builder.index(
    /// val[0], val[1], val[2], val[3]),
    /// val[4], val[5])
    /// ```
    A7654d351,
    /// 使用箇所: ruby27 58 (expr_value_do), ruby30 60 (expr_value_do), ruby31 64 (expr_value_do), ruby32 64 (expr_value_do) and 2 more.
    ///
    /// ```ruby
    /// @lexer.cond.pop
    /// result = [ val[1], val[2] ]
    /// ```
    A77d3fc34,
    /// 使用箇所: ruby27 367 (f_margs), ruby30 373 (f_margs), ruby30 495 (p_args_tail), ruby31 380 (f_margs) and 7 more.
    ///
    /// ```ruby
    /// result = [ val[0], *val[2] ]
    /// ```
    A7a88e600,
    /// 使用箇所: ruby27 684 (assoc), ruby30 697 (assoc), ruby31 712 (assoc), ruby32 710 (assoc) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.pair_keyword(val[0], val[1])
    /// ```
    A7acb1498,
    /// 使用箇所: ruby27 328 (@12).
    ///
    /// ```ruby
    /// local_push
    /// @context.in_class = true
    /// ```
    A7ad4b3e1,
    /// 使用箇所: ruby27 448 (p_top_expr_body), ruby30 456 (p_top_expr_body), ruby31 465 (p_top_expr_body), ruby32 467 (p_top_expr_body) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.array_pattern(nil, [val[0]].concat(val[2]), nil)
    /// ```
    A7b136a4c,
    /// 使用箇所: ruby27 269 (call_args), ruby30 279 (call_args), ruby31 284 (call_args), ruby32 284 (call_args) and 2 more.
    ///
    /// ```ruby
    /// result = [ @builder.associate(nil, val[0], nil) ]
    /// result.concat(val[1])
    /// ```
    A7b25f037,
    /// 使用箇所: ruby27 9 (top_stmt), ruby30 9 (top_stmt), ruby31 9 (top_stmt), ruby32 9 (top_stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.preexe(val[0], *val[1])
    /// ```
    A7bb825c5,
    /// 使用箇所: ruby27 82 (mlhs_basic), ruby30 88 (mlhs_basic), ruby31 92 (mlhs_basic), ruby32 92 (mlhs_basic) and 2 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// push(val[1])
    /// ```
    A7c3b3cb3,
    /// 使用箇所: ruby31 637 (f_paren_args), ruby32 635 (f_paren_args), ruby33 636 (f_paren_args), ruby34 636 (f_paren_args).
    ///
    /// ```ruby
    /// result = @builder.args(val[0], val[1], val[2])
    /// @lexer.state = :expr_value
    /// @context.in_argdef = false
    /// ```
    A7c8496ab,
    /// 使用箇所: ruby31 350 (primary), ruby32 352 (primary), ruby33 350 (primary), ruby34 350 (primary).
    ///
    /// ```ruby
    /// def_t, (name_t, ctx) = val[0]
    /// result = @builder.def_method(def_t, name_t, val[1],
    /// val[2], val[3])
    /// local_pop
    /// @current_arg_stack.pop
    /// @context.in_def = ctx.in_def
    /// ```
    A7e7960a4,
    /// 使用箇所: ruby27 475 (p_args), ruby30 486 (p_args), ruby31 496 (p_args), ruby32 498 (p_args) and 5 more.
    ///
    /// ```ruby
    /// result = [ *val[0], val[1] ]
    /// ```
    A7f1063c7,
    /// 使用箇所: ruby27 490 (p_kwargs), ruby27 494 (p_kwargs), ruby27 495 (p_kwargs), ruby30 502 (p_kwargs) and 4 more.
    ///
    /// ```ruby
    /// result = [ *val[0], *val[2] ]
    /// ```
    A804b401c,
    /// 使用箇所: ruby27 593 (keyword_variable), ruby30 604 (keyword_variable), ruby31 619 (keyword_variable), ruby32 617 (keyword_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.nil(val[0])
    /// ```
    A828a5dcf,
    /// 使用箇所: ruby31 640 (f_arglist), ruby32 638 (f_arglist), ruby33 639 (f_arglist), ruby34 639 (f_arglist).
    ///
    /// ```ruby
    /// @context.in_kwarg = val[0].in_kwarg
    /// @context.in_argdef = false
    /// result = @builder.args(nil, val[1], nil)
    /// ```
    A82bbe48e,
    /// 使用箇所: ruby27 395 (block_param_def), ruby30 403 (block_param_def).
    ///
    /// ```ruby
    /// @max_numparam_stack.has_ordinary_params!
    /// @current_arg_stack.set(nil)
    /// result = @builder.args(val[0], val[1].concat(val[2]), val[3])
    /// ```
    A83e8c7e3,
    /// 使用箇所: ruby27 281 (mrhs_arg), ruby30 291 (mrhs_arg), ruby31 297 (mrhs_arg), ruby32 299 (mrhs_arg) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.array(nil, val[0], nil)
    /// ```
    A84e4c06e,
    /// 使用箇所: ruby27 344 (k_class), ruby27 345 (k_module), ruby30 350 (k_class), ruby30 351 (k_module) and 8 more.
    ///
    /// ```ruby
    /// result = [ val[0], @context.dup ]
    /// ```
    A84fe4440,
    /// 使用箇所: ruby27 423 (method_call), ruby30 431 (method_call), ruby31 440 (method_call), ruby32 442 (method_call) and 2 more.
    ///
    /// ```ruby
    /// lparen_t, args, rparen_t = val[1]
    /// result = @builder.keyword_cmd(:super, val[0],
    /// lparen_t, args, rparen_t)
    /// ```
    A8796c317,
    /// 使用箇所: ruby27 7 (top_stmts), ruby27 16 (stmts), ruby27 275 (opt_block_arg), ruby27 675 (opt_f_block_arg) and 20 more.
    ///
    /// ```ruby
    /// result = [ val[1] ]
    /// ```
    A88e3ed53,
    /// 使用箇所: ruby27 62 (block_command), ruby27 68 (command), ruby27 70 (command), ruby30 68 (block_command) and 14 more.
    ///
    /// ```ruby
    /// result = @builder.call_method(val[0], val[1], val[2],
    /// nil, val[3], nil)
    /// ```
    A8afa606e,
    /// 使用箇所: ruby27 368 (f_rest_marg), ruby30 374 (f_rest_marg), ruby31 381 (f_rest_marg), ruby32 383 (f_rest_marg) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.restarg(val[0], val[1])
    /// ```
    A8bcedf58,
    /// 使用箇所: ruby27 359 (f_marg), ruby30 365 (f_marg), ruby31 372 (f_marg), ruby32 374 (f_marg) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.arg(val[0])
    /// ```
    A8cf4c799,
    /// 使用箇所: ruby27 431 (brace_body), ruby30 439 (brace_body), ruby31 448 (brace_body), ruby32 450 (brace_body) and 2 more.
    ///
    /// ```ruby
    /// args = @max_numparam_stack.has_numparams? ? @builder.numargs(@max_numparam_stack.top) : val[1]
    /// result = [ args, val[2] ]
    /// @max_numparam_stack.pop
    /// @static_env.unextend
    /// ```
    A8dc56848,
    /// 使用箇所: ruby27 523 (p_primitive), ruby27 600 (var_ref), ruby27 601 (var_ref), ruby30 535 (p_primitive) and 16 more.
    ///
    /// ```ruby
    /// result = @builder.accessible(val[0])
    /// ```
    A8dd4ceb8,
    /// 使用箇所: ruby27 488 (p_args_post), ruby27 497 (p_kwarg), ruby30 500 (p_args_post), ruby30 507 (p_kwarg) and 8 more.
    ///
    /// ```ruby
    /// result = [ *val[0], val[2] ]
    /// ```
    A8dfcf5c0,
    /// 使用箇所: ruby27 312 (primary), ruby30 322 (primary).
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:defined?, val[0],
    /// val[2], [ val[3] ], val[4])
    /// ```
    A8e7d7d50,
    /// 使用箇所: ruby27 484 (p_args_tail).
    ///
    /// ```ruby
    /// match_rest = @builder.match_rest(val[0], val[1])
    /// result = [ match_rest, *val[3] ]
    /// ```
    A8f09b359,
    /// 使用箇所: ruby31 677 (f_kw), ruby32 675 (f_kw), ruby33 676 (f_kw), ruby34 676 (f_kw).
    ///
    /// ```ruby
    /// @current_arg_stack.set(nil)
    /// @context.in_argdef = true
    /// result = @builder.kwarg(val[0])
    /// ```
    A8ffb7ec4,
    /// 使用箇所: ruby27 115 (cname), ruby30 121 (cname), ruby31 125 (cname), ruby32 125 (cname) and 2 more.
    ///
    /// ```ruby
    /// diagnostic :error, :module_name_const, nil, val[0]
    /// ```
    A90a5256a,
    /// 使用箇所: ruby27 402 (@18), ruby27 430 (@25), ruby27 432 (@26), ruby30 438 (@23) and 9 more.
    ///
    /// ```ruby
    /// @static_env.extend_dynamic
    /// @max_numparam_stack.push(static: false)
    /// ```
    A919593e1,
    /// 使用箇所: ruby27 11 (bodystmt), ruby30 11 (bodystmt), ruby31 11 (bodystmt), ruby32 11 (bodystmt) and 2 more.
    ///
    /// ```ruby
    /// rescue_bodies     = val[1]
    /// else_t,   else_   = val[2]
    /// ensure_t, ensure_ = val[3]
    /// if rescue_bodies.empty? && !else_t.nil?
    /// diagnostic :error, :useless_else, nil, else_t
    /// end
    /// result = @builder.begin_body(val[0],
    /// rescue_bodies,
    /// else_t,   else_,
    /// ensure_t, ensure_)
    /// ```
    A927a96c0,
    /// 使用箇所: ruby27 460 (p_expr_basic), ruby27 463 (p_expr_basic), ruby30 470 (p_expr_basic), ruby30 474 (p_expr_basic) and 8 more.
    ///
    /// ```ruby
    /// @pattern_hash_keys.pop
    /// pattern = @builder.hash_pattern(nil, val[2], nil)
    /// result = @builder.const_pattern(val[0], val[1], pattern, val[3])
    /// ```
    A929a25df,
    /// 使用箇所: ruby27 66 (command), ruby30 72 (command), ruby31 76 (command), ruby32 76 (command) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.call_method(nil, nil, val[0],
    /// nil, val[1], nil)
    /// ```
    A92f71b83,
    /// 使用箇所: ruby31 689 (f_opt), ruby31 690 (f_block_opt), ruby32 687 (f_opt), ruby32 688 (f_block_opt) and 4 more.
    ///
    /// ```ruby
    /// @current_arg_stack.set(0)
    /// @context.in_argdef = true
    /// result = @builder.optarg(val[0], val[1], val[2])
    /// ```
    A92fe5e25,
    /// 使用箇所: ruby27 643 (f_arg_asgn), ruby30 656 (f_arg_asgn), ruby31 670 (f_arg_asgn), ruby32 668 (f_arg_asgn) and 2 more.
    ///
    /// ```ruby
    /// @current_arg_stack.set(val[0][0])
    /// result = val[0]
    /// ```
    A9338e9b7,
    /// 使用箇所: ruby27 338 (primary).
    ///
    /// ```ruby
    /// result = @builder.def_singleton(val[0], val[1], val[2],
    /// val[4], val[6], val[7], val[8])
    /// local_pop
    /// @context.in_def = val[5].in_def
    /// @current_arg_stack.pop
    /// ```
    A93c61da1,
    /// 使用箇所: ruby27 443 (p_top_expr), ruby30 451 (p_top_expr), ruby31 460 (p_top_expr), ruby32 462 (p_top_expr) and 2 more.
    ///
    /// ```ruby
    /// result = [ val[0], nil ]
    /// ```
    A93e7323c,
    /// 使用箇所: ruby27 333 (primary), ruby30 342 (primary), ruby31 349 (primary), ruby32 351 (primary) and 1 more.
    ///
    /// ```ruby
    /// k_mod, ctx = val[0]
    /// if @context.in_def
    /// diagnostic :error, :module_in_def, nil, k_mod
    /// end
    /// result = @builder.def_module(k_mod, val[1],
    /// val[3], val[4])
    /// local_pop
    /// @context.in_class = ctx.in_class
    /// ```
    A9409bb42,
    /// 使用箇所: ruby27 576 (string_dvar), ruby27 592 (user_variable), ruby30 587 (string_dvar), ruby30 603 (user_variable) and 7 more.
    ///
    /// ```ruby
    /// result = @builder.cvar(val[0])
    /// ```
    A9435e2b9,
    /// 使用箇所: ruby27 683 (assoc), ruby30 696 (assoc), ruby31 711 (assoc), ruby32 709 (assoc) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.pair(val[0], val[1], val[2])
    /// ```
    A94577fdc,
    /// 使用箇所: ruby27 101 (mlhs_node), ruby27 109 (lhs), ruby27 110 (lhs), ruby27 111 (lhs) and 20 more.
    ///
    /// ```ruby
    /// result = @builder.attr_asgn(val[0], val[1], val[2])
    /// ```
    A952c1f18,
    /// 使用箇所: ruby27 639 (f_bad_arg), ruby30 652 (f_bad_arg), ruby31 666 (f_bad_arg), ruby32 664 (f_bad_arg) and 2 more.
    ///
    /// ```ruby
    /// diagnostic :error, :argument_gvar, nil, val[0]
    /// ```
    A95689d9e,
    /// 使用箇所: ruby27 340 (primary), ruby30 346 (primary), ruby31 353 (primary), ruby32 355 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:next, val[0])
    /// ```
    A966b19eb,
    /// 使用箇所: ruby27 259 (paren_args), ruby30 269 (paren_args), ruby31 274 (paren_args), ruby32 274 (paren_args) and 2 more.
    ///
    /// ```ruby
    /// unless @static_env.declared_forward_args?
    /// diagnostic :error, :unexpected_token, { :token => 'tBDOT3' } , val[1]
    /// end
    /// result = [val[0], [@builder.forwarded_args(val[1])], val[2]]
    /// ```
    A9819c4f1,
    /// 使用箇所: ruby31 549 (p_var_ref), ruby32 549 (p_var_ref), ruby33 549 (p_var_ref), ruby34 549 (p_var_ref).
    ///
    /// ```ruby
    /// non_lvar = @builder.accessible(val[1])
    /// result = @builder.pin(val[0], non_lvar)
    /// ```
    A984fe56d,
    /// 使用箇所: ruby27 29 (stmt), ruby27 46 (command_rhs), ruby27 256 (arg_rhs), ruby30 29 (stmt) and 18 more.
    ///
    /// ```ruby
    /// rescue_body = @builder.rescue_body(val[1],
    /// nil, nil, nil,
    /// nil, val[2])
    /// result = @builder.begin_body(val[0], [ rescue_body ])
    /// ```
    A9865da41,
    /// 使用箇所: ruby27 660 (f_kwrest), ruby30 673 (f_kwrest), ruby31 687 (f_kwrest), ruby32 685 (f_kwrest) and 2 more.
    ///
    /// ```ruby
    /// @static_env.declare val[1][0]
    /// result = [ @builder.kwrestarg(val[0], val[1]) ]
    /// ```
    A98ddbbba,
    /// 使用箇所: ruby30 497 (p_rest), ruby31 507 (p_rest), ruby32 507 (p_rest), ruby33 507 (p_rest) and 1 more.
    ///
    /// ```ruby
    /// result = @builder.match_rest(val[0], val[1])
    /// ```
    A999204e9,
    /// 使用箇所: ruby27 700 (call_op), ruby30 713 (call_op), ruby31 729 (call_op), ruby32 726 (call_op) and 2 more.
    ///
    /// ```ruby
    /// result = [:anddot, val[0][1]]
    /// ```
    A99e1041d,
    /// 使用箇所: ruby27 300 (primary), ruby30 310 (primary), ruby31 316 (primary), ruby32 318 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.begin(val[0], val[1], val[3])
    /// ```
    A9aa9e915,
    /// 使用箇所: ruby27 640 (f_bad_arg), ruby30 653 (f_bad_arg), ruby31 667 (f_bad_arg), ruby32 665 (f_bad_arg) and 2 more.
    ///
    /// ```ruby
    /// diagnostic :error, :argument_cvar, nil, val[0]
    /// ```
    A9e76b419,
    /// 使用箇所: ruby30 250 (arg).
    ///
    /// ```ruby
    /// def_t, (name_t, ctx) = val[0]
    /// endless_method_name(name_t)
    /// rescue_body = @builder.rescue_body(val[4],
    /// nil, nil, nil,
    /// nil, val[5])
    /// method_body = @builder.begin_body(val[3], [ rescue_body ])
    /// result = @builder.def_endless_method(def_t, name_t,
    /// val[1], val[2], method_body)
    /// local_pop
    /// @context.in_def = ctx.in_def
    /// @current_arg_stack.pop
    /// ```
    A9e773905,
    /// 使用箇所: ruby27 77 (mlhs), ruby27 79 (mlhs_inner), ruby30 83 (mlhs), ruby30 85 (mlhs_inner) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.multi_lhs(nil, val[0], nil)
    /// ```
    A9ed60617,
    /// 使用箇所: ruby27 335 (primary).
    ///
    /// ```ruby
    /// result = @builder.def_method(val[0], val[1],
    /// val[3], val[4], val[5])
    /// local_pop
    /// @context.in_def = val[2].in_def
    /// @current_arg_stack.pop
    /// ```
    Aa0590d4d,
    /// 使用箇所: ruby27 2 (program), ruby30 2 (program), ruby31 2 (program), ruby32 2 (program) and 2 more.
    ///
    /// ```ruby
    /// result = val[1]
    /// @current_arg_stack.pop
    /// @max_numparam_stack.pop
    /// ```
    Aa07db756,
    /// 使用箇所: ruby27 270 (call_args), ruby30 280 (call_args), ruby31 285 (call_args), ruby32 285 (call_args) and 2 more.
    ///
    /// ```ruby
    /// assocs = @builder.associate(nil, val[2], nil)
    /// result = val[0] << assocs
    /// result.concat(val[3])
    /// ```
    Aa0f2ecf7,
    /// 使用箇所: ruby27 552 (word_list), ruby27 557 (symbol_list), ruby30 563 (word_list), ruby30 568 (symbol_list) and 8 more.
    ///
    /// ```ruby
    /// result = val[0] << @builder.word(val[1])
    /// ```
    Aa2cb7f46,
    /// 使用箇所: ruby27 104 (mlhs_node), ruby27 113 (lhs), ruby30 110 (mlhs_node), ruby30 119 (lhs) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.assignable(
    /// @builder.const_global(val[0], val[1]))
    /// ```
    Aa2eaed0a,
    /// 使用箇所: ruby27 341 (primary), ruby30 347 (primary), ruby31 354 (primary), ruby32 356 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:redo, val[0])
    /// ```
    Aa43fad97,
    /// 使用箇所: ruby27 215 (arg), ruby27 514 (p_value), ruby30 221 (arg), ruby30 526 (p_value) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.range_exclusive(nil, val[0], val[1])
    /// ```
    Aa46e321a,
    /// 使用箇所: ruby27 254 (aref_args), ruby27 266 (opt_call_args), ruby30 264 (aref_args), ruby30 276 (opt_call_args) and 8 more.
    ///
    /// ```ruby
    /// result = [ @builder.associate(nil, val[0], nil) ]
    /// ```
    Aa47e4446,
    /// 使用箇所: ruby27 477 (p_args), ruby30 488 (p_args), ruby31 498 (p_args).
    ///
    /// ```ruby
    /// match_rest = @builder.match_rest(val[1], val[2])
    /// result = [ *val[0], match_rest, *val[4] ]
    /// ```
    Aa870de87,
    /// 使用箇所: ruby27 69 (command), ruby27 71 (command), ruby27 416 (block_call), ruby30 75 (command) and 14 more.
    ///
    /// ```ruby
    /// method_call = @builder.call_method(val[0], val[1], val[2],
    /// nil, val[3], nil)
    /// begin_t, args, body, end_t = val[4]
    /// result      = @builder.block(method_call,
    /// begin_t, args, body, end_t)
    /// ```
    Aa908316f,
    /// 使用箇所: ruby27 23 (stmt), ruby30 23 (stmt), ruby31 23 (stmt), ruby32 23 (stmt) and 2 more.
    ///
    /// ```ruby
    /// diagnostic :error, :nth_ref_alias, nil, val[2]
    /// ```
    Aaa5cbd81,
    /// 使用箇所: ruby33 58 (expr), ruby34 58 (expr).
    ///
    /// ```ruby
    /// @pattern_variables.pop
    /// @pattern_hash_keys.pop
    /// @context.in_kwarg = val[2]
    /// result = @builder.match_pattern(val[0], val[1], val[5])
    /// ```
    Aaa781827,
    /// 使用箇所: ruby31 676 (f_kw), ruby32 674 (f_kw), ruby33 675 (f_kw), ruby34 675 (f_kw).
    ///
    /// ```ruby
    /// @current_arg_stack.set(nil)
    /// @context.in_argdef = true
    /// result = @builder.kwoptarg(val[0], val[1])
    /// ```
    Aaa863d99,
    /// 使用箇所: ruby33 457 (p_in_kwarg), ruby34 457 (p_in_kwarg).
    ///
    /// ```ruby
    /// result = @context.in_kwarg
    /// @lexer.state = :expr_beg
    /// @lexer.command_start = false
    /// @context.in_kwarg = true
    /// ```
    Aaa931f77,
    /// 使用箇所: ruby27 500 (p_kw_label), ruby30 510 (p_kw_label), ruby31 520 (p_kw_label), ruby32 520 (p_kw_label) and 2 more.
    ///
    /// ```ruby
    /// result = [:label, val[0]]
    /// ```
    Aab259dbd,
    /// 使用箇所: ruby27 420 (method_call), ruby30 428 (method_call), ruby31 437 (method_call), ruby32 439 (method_call) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.call_method(val[0], val[1], val[2])
    /// ```
    Aab3f74d7,
    /// 使用箇所: ruby27 574 (string_dvar), ruby27 590 (user_variable), ruby30 585 (string_dvar), ruby30 601 (user_variable) and 7 more.
    ///
    /// ```ruby
    /// result = @builder.gvar(val[0])
    /// ```
    Aac32000a,
    /// 使用箇所: ruby27 52 (expr), ruby27 235 (arg), ruby30 52 (expr), ruby30 241 (arg) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.not_op(val[0], nil, val[1], nil)
    /// ```
    Aacb86ab1,
    /// 使用箇所: ruby27 393 (opt_block_param), ruby27 606 (@35), ruby30 401 (opt_block_param), ruby30 617 (@31) and 8 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_value
    /// ```
    Aad0ee754,
    /// 使用箇所: ruby27 332 (@14), ruby30 337 (@13), ruby30 341 (@15), ruby31 344 (@15) and 5 more.
    ///
    /// ```ruby
    /// @context.in_class = true
    /// local_push
    /// ```
    Aad6896ab,
    /// 使用箇所: ruby31 252 (arg), ruby32 252 (arg).
    ///
    /// ```ruby
    /// @context.in_defined = false
    /// result = @builder.keyword_cmd(:defined?, val[0], nil, [ val[3] ], nil)
    /// ```
    Aadbeb0cd,
    /// 使用箇所: ruby27 648 (f_label), ruby30 661 (f_label).
    ///
    /// ```ruby
    /// check_kwarg_name(val[0])
    /// @static_env.declare val[0][0]
    /// @max_numparam_stack.has_ordinary_params!
    /// @current_arg_stack.set(val[0][0])
    /// result = val[0]
    /// ```
    Aae6cc7c6,
    /// 使用箇所: ruby27 330 (@13), ruby30 339 (@14), ruby31 346 (@16), ruby32 348 (@16) and 1 more.
    ///
    /// ```ruby
    /// @context.in_def = false
    /// @context.in_class = false
    /// local_push
    /// ```
    Aaeaf9e48,
    /// 使用箇所: ruby27 434 (do_body), ruby30 442 (do_body), ruby31 451 (do_body), ruby32 453 (do_body) and 2 more.
    ///
    /// ```ruby
    /// args = @max_numparam_stack.has_numparams? ? @builder.numargs(@max_numparam_stack.top) : val[2]
    /// result = [ args, val[3] ]
    /// @max_numparam_stack.pop
    /// @static_env.unextend
    /// @lexer.cmdarg.pop
    /// ```
    Aaebcec57,
    /// 使用箇所: ruby27 549 (regexp), ruby30 560 (regexp), ruby31 572 (regexp), ruby32 572 (regexp) and 2 more.
    ///
    /// ```ruby
    /// opts   = @builder.regexp_options(val[3])
    /// result = @builder.regexp_compose(val[0], val[1], val[2], opts)
    /// ```
    Ab0c84dc6,
    /// 使用箇所: ruby27 315 (primary), ruby30 325 (primary), ruby31 332 (primary), ruby32 334 (primary) and 2 more.
    ///
    /// ```ruby
    /// method_call = @builder.call_method(nil, nil, val[0])
    /// begin_t, args, body, end_t = val[1]
    /// result      = @builder.block(method_call,
    /// begin_t, args, body, end_t)
    /// ```
    Ab149ce13,
    /// 使用箇所: ruby32 500 (p_args), ruby33 500 (p_args), ruby34 500 (p_args).
    ///
    /// ```ruby
    /// result = [ *val[0], val[1], *val[3] ]
    /// ```
    Ab1e5b34e,
    /// 使用箇所: ruby27 350 (then), ruby27 375 (opt_block_args_tail), ruby27 571 (string_content), ruby27 619 (opt_args_tail) and 48 more.
    ///
    /// ```ruby
    /// result = val[1]
    /// ```
    Ab39c3161,
    /// 使用箇所: ruby31 550 (p_expr_ref), ruby32 550 (p_expr_ref), ruby33 550 (p_expr_ref), ruby34 550 (p_expr_ref).
    ///
    /// ```ruby
    /// expr = @builder.begin(val[1], val[2], val[3])
    /// result = @builder.pin(val[0], expr)
    /// ```
    Ab4497d2f,
    /// 使用箇所: ruby27 298 (primary), ruby30 308 (primary), ruby31 314 (primary), ruby32 316 (primary) and 2 more.
    ///
    /// ```ruby
    /// @lexer.cmdarg.pop
    /// result = @builder.begin_keyword(val[0], val[2], val[3])
    /// ```
    Ab44e54e1,
    /// 使用箇所: ruby27 585 (simple_numeric), ruby30 596 (simple_numeric), ruby31 608 (simple_numeric), ruby32 608 (simple_numeric) and 2 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_end
    /// result = @builder.float(val[0])
    /// ```
    Ab4f3ce25,
    /// 使用箇所: ruby27 479 (p_args), ruby30 490 (p_args), ruby31 500 (p_args).
    ///
    /// ```ruby
    /// result = [ *val[0], @builder.match_rest(val[1]), *val[3] ]
    /// ```
    Ab53adfad,
    /// 使用箇所: ruby31 58 (expr), ruby32 58 (expr).
    ///
    /// ```ruby
    /// @pattern_variables.pop
    /// @pattern_hash_keys.pop
    /// @context.in_kwarg = val[2]
    /// result = @builder.match_pattern(val[0], val[1], val[3])
    /// ```
    Ab53deae7,
    /// 使用箇所: ruby27 117 (cpath), ruby27 305 (primary), ruby27 528 (p_const), ruby30 123 (cpath) and 14 more.
    ///
    /// ```ruby
    /// result = @builder.const_global(val[0], val[1])
    /// ```
    Ab55a6aa2,
    /// 使用箇所: ruby31 47 (command_asgn), ruby31 257 (arg), ruby32 47 (command_asgn), ruby32 257 (arg).
    ///
    /// ```ruby
    /// def_t, recv, dot_t, (name_t, ctx) = val[0]
    /// endless_method_name(name_t)
    /// rescue_body = @builder.rescue_body(val[4],
    /// nil, nil, nil,
    /// nil, val[5])
    /// method_body = @builder.begin_body(val[3], [ rescue_body ])
    /// result = @builder.def_endless_singleton(def_t, recv, dot_t, name_t,
    /// val[1], val[2], method_body)
    /// local_pop
    /// @current_arg_stack.pop
    /// @context.in_def = ctx.in_def
    /// ```
    Ab5e85d3c,
    /// 使用箇所: ruby27 42 (command_asgn), ruby27 207 (arg), ruby30 42 (command_asgn), ruby30 213 (arg) and 8 more.
    ///
    /// ```ruby
    /// const  = @builder.const_op_assignable(
    /// @builder.const_fetch(val[0], val[1], val[2]))
    /// result = @builder.op_assign(const, val[3], val[4])
    /// ```
    Ab6eab20c,
    /// 使用箇所: ruby31 44 (command_asgn), ruby31 254 (arg), ruby32 44 (command_asgn), ruby32 254 (arg) and 4 more.
    ///
    /// ```ruby
    /// def_t, (name_t, ctx) = val[0]
    /// endless_method_name(name_t)
    /// result = @builder.def_endless_method(def_t, name_t,
    /// val[1], val[2], val[3])
    /// local_pop
    /// @current_arg_stack.pop
    /// @context.in_def = ctx.in_def
    /// ```
    Ab7215c2b,
    /// 使用箇所: ruby27 450 (p_top_expr_body), ruby30 459 (p_top_expr_body), ruby31 468 (p_top_expr_body), ruby32 470 (p_top_expr_body) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.hash_pattern(nil, val[0], nil)
    /// ```
    Ab73c151c,
    /// 使用箇所: ruby27 118 (cpath), ruby27 530 (p_const), ruby27 591 (user_variable), ruby30 124 (cpath) and 14 more.
    ///
    /// ```ruby
    /// result = @builder.const(val[0])
    /// ```
    Ab7c8fc06,
    /// 使用箇所: ruby27 75 (command), ruby30 81 (command), ruby31 85 (command), ruby32 85 (command) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:break, val[0],
    /// nil, val[1], nil)
    /// ```
    Ab818b18a,
    /// 使用箇所: ruby27 581 (dsym), ruby30 592 (dsym), ruby31 604 (dsym), ruby32 604 (dsym) and 2 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_end
    /// result = @builder.symbol_compose(val[0], val[1], val[2])
    /// ```
    Ab818c837,
    /// 使用箇所: ruby27 609 (f_arglist), ruby30 622 (f_paren_args).
    ///
    /// ```ruby
    /// result = @builder.args(val[0], val[1], val[2])
    /// @lexer.state = :expr_value
    /// ```
    Aba094cd0,
    /// 使用箇所: ruby27 413 (block_call), ruby30 421 (block_call), ruby31 430 (block_call), ruby32 432 (block_call) and 2 more.
    ///
    /// ```ruby
    /// begin_t, block_args, body, end_t = val[1]
    /// result      = @builder.block(val[0],
    /// begin_t, block_args, body, end_t)
    /// ```
    Aba22ce31,
    /// 使用箇所: ruby27 550 (words), ruby27 558 (qwords), ruby30 561 (words), ruby30 569 (qwords) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.words_compose(val[0], val[1], val[2])
    /// ```
    Abb2224f9,
    /// 使用箇所: ruby27 129 (undef_list), ruby30 135 (undef_list), ruby31 139 (undef_list), ruby32 139 (undef_list) and 2 more.
    ///
    /// ```ruby
    /// result = val[0] << val[3]
    /// ```
    Abbd434f2,
    /// 使用箇所: ruby27 387 (block_param), ruby27 389 (block_param), ruby27 627 (f_args), ruby27 630 (f_args) and 26 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// concat(val[1])
    /// ```
    Abc102fa6,
    /// 使用箇所: ruby27 377 (block_param), ruby27 380 (block_param), ruby27 383 (block_param), ruby27 386 (block_param) and 44 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// concat(val[2]).
    /// concat(val[4]).
    /// concat(val[5])
    /// ```
    Abc1f4cef,
    /// 使用箇所: ruby31 639 (@35), ruby32 637 (@35), ruby33 638 (@29), ruby34 638 (@29).
    ///
    /// ```ruby
    /// result = @context.dup
    /// @context.in_kwarg = true
    /// @context.in_argdef = true
    /// ```
    Abd0b6006,
    /// 使用箇所: ruby27 222 (arg), ruby30 228 (arg), ruby31 232 (arg), ruby32 232 (arg) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.unary_op(val[0],
    /// @builder.binary_op(
    /// val[1], val[2], val[3]))
    /// ```
    Abd6baefa,
    /// 使用箇所: ruby27 319 (primary), ruby27 525 (p_primitive).
    ///
    /// ```ruby
    /// lambda_call = @builder.call_lambda(val[0])
    /// args, (begin_t, body, end_t) = val[2]
    /// result      = @builder.block(lambda_call,
    /// begin_t, args, body, end_t)
    /// @context.in_lambda = val[1].in_lambda
    /// ```
    Abdf4b1f0,
    /// 使用箇所: ruby33 455 (p_pvtbl), ruby34 455 (p_pvtbl).
    ///
    /// ```ruby
    /// @pattern_variables.push
    /// ```
    Abe7fd930,
    /// 使用箇所: ruby27 572 (@34), ruby30 583 (@30), ruby31 595 (@33), ruby32 595 (@33) and 2 more.
    ///
    /// ```ruby
    /// @lexer.cmdarg.push(false)
    /// @lexer.cond.push(false)
    /// ```
    Abec2ebf1,
    /// 使用箇所: ruby27 51 (expr), ruby30 51 (expr), ruby31 55 (expr), ruby32 55 (expr) and 6 more.
    ///
    /// ```ruby
    /// result = @builder.not_op(val[0], nil, val[2], nil)
    /// ```
    Abf073d81,
    /// 使用箇所: ruby30 249 (arg).
    ///
    /// ```ruby
    /// def_t, (name_t, ctx) = val[0]
    /// endless_method_name(name_t)
    /// result = @builder.def_endless_method(def_t, name_t,
    /// val[1], val[2], val[3])
    /// local_pop
    /// @context.in_def = ctx.in_def
    /// @current_arg_stack.pop
    /// ```
    Abf19aa1e,
    /// 使用箇所: ruby27 331 (primary), ruby30 340 (primary), ruby31 347 (primary), ruby32 349 (primary) and 1 more.
    ///
    /// ```ruby
    /// k_class, ctx = val[0]
    /// result = @builder.def_sclass(k_class, val[1], val[2],
    /// val[5], val[6])
    /// local_pop
    /// @context.in_def = ctx.in_def
    /// @context.in_class = ctx.in_class
    /// ```
    Abf2cb3ce,
    /// 使用箇所: ruby27 309 (primary), ruby30 319 (primary), ruby31 325 (primary), ruby32 327 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:yield, val[0], val[1], val[2], val[3])
    /// ```
    Abf57b483,
    /// 使用箇所: ruby30 477 (p_expr_basic), ruby31 487 (p_expr_basic), ruby32 489 (p_expr_basic), ruby33 489 (p_expr_basic) and 1 more.
    ///
    /// ```ruby
    /// result = @builder.find_pattern(val[0], val[1], val[2])
    /// ```
    Abf85f6ed,
    /// 使用箇所: ruby27 469 (p_expr_basic), ruby30 480 (p_expr_basic), ruby31 490 (p_expr_basic), ruby32 492 (p_expr_basic) and 2 more.
    ///
    /// ```ruby
    /// @pattern_hash_keys.pop
    /// @context.in_kwarg = val[1]
    /// result = @builder.hash_pattern(val[0], val[2], val[3])
    /// ```
    Ac06d0a92,
    /// 使用箇所: ruby30 412 (lambda), ruby31 421 (lambda), ruby32 423 (lambda), ruby33 421 (lambda) and 1 more.
    ///
    /// ```ruby
    /// lambda_call = @builder.call_lambda(val[0])
    /// args = @max_numparam_stack.has_numparams? ? @builder.numargs(@max_numparam_stack.top) : val[2]
    /// begin_t, body, end_t = val[4]
    /// @max_numparam_stack.pop
    /// @static_env.unextend
    /// @lexer.cmdarg.pop
    /// @context.in_lambda = val[1].in_lambda
    /// result      = @builder.block(lambda_call,
    /// begin_t, args, body, end_t)
    /// ```
    Ac0be2825,
    /// 使用箇所: ruby27 311 (primary), ruby30 321 (primary), ruby31 327 (primary), ruby32 329 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:yield, val[0])
    /// ```
    Ac0c08ce5,
    /// 使用箇所: ruby30 64 (defs_head), ruby31 68 (defs_head), ruby32 68 (defs_head), ruby33 67 (defs_head) and 1 more.
    ///
    /// ```ruby
    /// result = [ val[0], val[1], val[2], val[4] ]
    /// ```
    Ac1c7810d,
    /// 使用箇所: ruby27 467 (p_expr_basic), ruby30 478 (p_expr_basic), ruby31 488 (p_expr_basic), ruby32 490 (p_expr_basic) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.array_pattern(val[0], [], val[1])
    /// ```
    Ac322857e,
    /// 使用箇所: ruby27 583 (numeric), ruby30 594 (numeric), ruby31 606 (numeric), ruby32 606 (numeric) and 2 more.
    ///
    /// ```ruby
    /// if @builder.respond_to? :negate
    /// result = @builder.negate(val[0], val[1])
    /// else
    /// result = @builder.unary_num(val[0], val[1])
    /// end
    /// ```
    Ac47cc224,
    /// 使用箇所: ruby27 233 (arg), ruby30 239 (arg), ruby31 243 (arg), ruby32 243 (arg) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.match_op(val[0], val[1], val[2])
    /// ```
    Ac5931e06,
    /// 使用箇所: ruby27 280 (args), ruby27 284 (mrhs), ruby30 290 (args), ruby30 294 (mrhs) and 6 more.
    ///
    /// ```ruby
    /// result = val[0] << @builder.splat(val[2], val[3])
    /// ```
    Ac7a9f196,
    /// 使用箇所: ruby31 411 (block_param_def), ruby32 413 (block_param_def), ruby33 411 (block_param_def), ruby34 411 (block_param_def).
    ///
    /// ```ruby
    /// @max_numparam_stack.has_ordinary_params!
    /// @current_arg_stack.set(nil)
    /// @context.in_argdef = false
    /// result = @builder.args(val[0], val[1], val[2])
    /// ```
    Ac8298278,
    /// 使用箇所: ruby34 347 (primary).
    ///
    /// ```ruby
    /// k_class, ctx = val[0]
    /// result = @builder.def_sclass(k_class, val[1], val[2],
    /// val[5], val[6])
    /// local_pop
    /// @context.in_def = ctx.in_def
    /// @context.in_class = ctx.in_class
    /// @context.cant_return = ctx.cant_return
    /// ```
    Ac990afce,
    /// 使用箇所: ruby27 546 (string1), ruby30 557 (string1), ruby31 569 (string1), ruby32 569 (string1) and 2 more.
    ///
    /// ```ruby
    /// string = @builder.string(val[0])
    /// result = @builder.dedent_string(string, @lexer.dedent_level)
    /// ```
    Aca1323ae,
    /// 使用箇所: ruby27 421 (method_call), ruby27 422 (method_call), ruby30 429 (method_call), ruby30 430 (method_call) and 8 more.
    ///
    /// ```ruby
    /// lparen_t, args, rparen_t = val[2]
    /// result = @builder.call_method(val[0], val[1], nil,
    /// lparen_t, args, rparen_t)
    /// ```
    Aca95bf6a,
    /// 使用箇所: ruby27 498 (p_kw), ruby30 508 (p_kw), ruby31 518 (p_kw), ruby32 518 (p_kw) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.match_pair(*val[0], val[1])
    /// ```
    Acb037965,
    /// 使用箇所: ruby27 370 (block_args_tail), ruby27 614 (args_tail), ruby30 378 (block_args_tail), ruby30 628 (args_tail) and 8 more.
    ///
    /// ```ruby
    /// result = val[0].concat(val[2]).concat(val[3])
    /// ```
    Acc8cb959,
    /// 使用箇所: ruby27 57 (@4), ruby30 59 (@5), ruby31 63 (@5), ruby32 63 (@5) and 2 more.
    ///
    /// ```ruby
    /// @lexer.cond.push(true)
    /// ```
    Accd7b4c6,
    /// 使用箇所: ruby27 78 (mlhs), ruby27 92 (mlhs_item), ruby27 303 (primary), ruby30 84 (mlhs) and 14 more.
    ///
    /// ```ruby
    /// result = @builder.begin(val[0], val[1], val[2])
    /// ```
    Accdcd3c7,
    /// 使用箇所: ruby33 329 (primary), ruby34 329 (primary).
    ///
    /// ```ruby
    /// @context.in_defined = val[3].in_defined
    /// result = @builder.keyword_cmd(:defined?, val[0],
    /// val[2], [ val[4] ], val[5])
    /// ```
    Acd4e17b7,
    /// 使用箇所: ruby27 44 (command_asgn), ruby30 44 (command_asgn), ruby31 48 (command_asgn), ruby32 48 (command_asgn) and 2 more.
    ///
    /// ```ruby
    /// @builder.op_assign(val[0], val[1], val[2])
    /// ```
    Acde2b4f8,
    /// 使用箇所: ruby27 686 (assoc), ruby30 699 (assoc), ruby31 715 (assoc), ruby32 713 (assoc) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.kwsplat(val[0], val[1])
    /// ```
    Ace674ac2,
    /// 使用箇所: ruby34 345 (primary).
    ///
    /// ```ruby
    /// k_class, ctx = val[0]
    /// if @context.in_def
    /// diagnostic :error, :class_in_def, nil, k_class
    /// end
    /// lt_t, superclass = val[2]
    /// result = @builder.def_class(k_class, val[1],
    /// lt_t, superclass,
    /// val[4], val[5])
    /// local_pop
    /// @context.in_class = ctx.in_class
    /// @context.cant_return = ctx.cant_return
    /// ```
    Acef0ceee,
    /// 使用箇所: ruby32 295 (args).
    ///
    /// ```ruby
    /// if !@static_env.declared_anonymous_restarg?
    /// diagnostic :error, :no_anonymous_restarg, nil, val[0]
    /// end
    /// result = [ @builder.forwarded_restarg(val[0]) ]
    /// ```
    Acf0f54f0,
    /// 使用箇所: ruby27 72 (command), ruby30 78 (command), ruby31 82 (command), ruby32 82 (command) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:super, val[0],
    /// nil, val[1], nil)
    /// ```
    Acf42b7e1,
    /// 使用箇所: ruby27 318 (@11), ruby27 407 (@20), ruby27 409 (@21), ruby27 524 (@33) and 10 more.
    ///
    /// ```ruby
    /// result = @context.dup
    /// @context.in_lambda = true
    /// ```
    Acfb4c3e6,
    /// 使用箇所: ruby27 575 (string_dvar), ruby27 589 (user_variable), ruby30 586 (string_dvar), ruby30 600 (user_variable) and 7 more.
    ///
    /// ```ruby
    /// result = @builder.ivar(val[0])
    /// ```
    Acff17c35,
    /// 使用箇所: ruby27 10 (begin_block), ruby27 257 (paren_args), ruby27 356 (opt_else), ruby30 10 (begin_block) and 17 more.
    ///
    /// ```ruby
    /// result = val
    /// ```
    Ad02d1a27,
    /// 使用箇所: ruby27 456 (p_lparen), ruby27 457 (p_lbracket), ruby30 465 (p_lparen), ruby30 466 (p_lbracket) and 8 more.
    ///
    /// ```ruby
    /// result = val[0]
    /// @pattern_hash_keys.push
    /// ```
    Ad0565574,
    /// 使用箇所: ruby34 344 (@10), ruby34 348 (@12).
    ///
    /// ```ruby
    /// @context.in_class = true
    /// @context.cant_return = true
    /// local_push
    /// ```
    Ad09f05c7,
    /// 使用箇所: ruby27 584 (simple_numeric), ruby30 595 (simple_numeric), ruby31 607 (simple_numeric), ruby32 607 (simple_numeric) and 2 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_end
    /// result = @builder.integer(val[0])
    /// ```
    Ad118db01,
    /// 使用箇所: ruby30 1 (@1), ruby31 1 (@1), ruby32 1 (@1), ruby33 1 (@1) and 1 more.
    ///
    /// ```ruby
    /// @current_arg_stack.push(nil)
    /// @max_numparam_stack.push(static: true)
    /// ```
    Ad173a028,
    /// 使用箇所: ruby27 548 (xstring), ruby30 559 (xstring), ruby31 571 (xstring), ruby32 571 (xstring) and 2 more.
    ///
    /// ```ruby
    /// string = @builder.xstring_compose(val[0], val[1], val[2])
    /// result = @builder.dedent_string(string, @lexer.dedent_level)
    /// ```
    Ad1d73703,
    /// 使用箇所: ruby33 291 (block_arg), ruby34 291 (block_arg).
    ///
    /// ```ruby
    /// if !@static_env.declared_anonymous_blockarg?
    /// diagnostic :error, :no_anonymous_blockarg, nil, val[0]
    /// end
    /// if @context.in_dynamic_block? && context.in_def &&
    /// @static_env.declared_anonymous_blockarg_in_current_scpe? && @static_env.parent_has_anonymous_blockarg?
    /// diagnostic :error, :ambiguous_anonymous_blockarg, nil, val[0]
    /// end
    /// result = @builder.block_pass(val[0], nil)
    /// ```
    Ad2270fea,
    /// 使用箇所: ruby27 271 (call_args), ruby30 281 (call_args), ruby31 286 (call_args), ruby32 286 (call_args) and 2 more.
    ///
    /// ```ruby
    /// result =  [ val[0] ]
    /// ```
    Ad26d5e74,
    /// 使用箇所: ruby27 94 (mlhs_head), ruby27 544 (string), ruby27 554 (word), ruby27 565 (string_contents) and 32 more.
    ///
    /// ```ruby
    /// result = val[0] << val[1]
    /// ```
    Ad4231615,
    /// 使用箇所: ruby30 496 (p_find), ruby31 506 (p_find), ruby32 506 (p_find), ruby33 506 (p_find) and 1 more.
    ///
    /// ```ruby
    /// result = [ val[0], *val[2], val[4] ]
    /// ```
    Ad47d3507,
    /// 使用箇所: ruby32 714 (assoc).
    ///
    /// ```ruby
    /// if !@static_env.declared_anonymous_kwrestarg?
    /// diagnostic :error, :no_anonymous_kwrestarg, nil, val[0]
    /// end
    /// result = @builder.forwarded_kwrestarg(val[0])
    /// ```
    Ad4aa53d7,
    /// 使用箇所: ruby27 563 (qsym_list), ruby30 574 (qsym_list), ruby31 586 (qsym_list), ruby32 586 (qsym_list) and 2 more.
    ///
    /// ```ruby
    /// result = val[0] << @builder.symbol_internal(val[1])
    /// ```
    Ad63fa348,
    /// 使用箇所: ruby27 76 (command), ruby30 82 (command), ruby31 86 (command), ruby32 86 (command) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:next, val[0],
    /// nil, val[1], nil)
    /// ```
    Ad6aa70f3,
    /// 使用箇所: ruby27 435 (case_body), ruby30 443 (case_body), ruby31 452 (case_body), ruby32 454 (case_body) and 2 more.
    ///
    /// ```ruby
    /// result = [ @builder.when(val[0], val[1], val[2], val[3]),
    /// *val[4] ]
    /// ```
    Ad8f6c640,
    /// 使用箇所: ruby27 306 (primary), ruby30 316 (primary), ruby31 322 (primary), ruby32 324 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.array(val[0], val[1], val[2])
    /// ```
    Ad9abff46,
    /// 使用箇所: ruby31 60 (expr), ruby32 60 (expr).
    ///
    /// ```ruby
    /// @pattern_variables.pop
    /// @pattern_hash_keys.pop
    /// @context.in_kwarg = val[2]
    /// result = @builder.match_pattern_p(val[0], val[1], val[3])
    /// ```
    Ad9fb89fb,
    /// 使用箇所: ruby30 457 (p_top_expr_body), ruby31 466 (p_top_expr_body), ruby32 468 (p_top_expr_body), ruby33 468 (p_top_expr_body) and 1 more.
    ///
    /// ```ruby
    /// result = @builder.find_pattern(nil, val[0], nil)
    /// ```
    Ada37bf1e,
    /// 使用箇所: ruby27 64 (cmd_brace_block), ruby27 412 (do_block), ruby27 427 (brace_block), ruby27 429 (brace_block) and 4 more.
    ///
    /// ```ruby
    /// result = [ val[0], *val[2], val[3] ]
    /// @context.in_block = val[1].in_block
    /// ```
    Ada877101,
    /// 使用箇所: ruby27 339 (primary), ruby30 345 (primary), ruby31 352 (primary), ruby32 354 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.keyword_cmd(:break, val[0])
    /// ```
    Adb9388e1,
    /// 使用箇所: ruby30 410 (@16), ruby31 419 (@19), ruby32 421 (@19), ruby33 419 (@14) and 1 more.
    ///
    /// ```ruby
    /// @static_env.extend_dynamic
    /// @max_numparam_stack.push(static: false)
    /// result = @context.dup
    /// @context.in_lambda = true
    /// ```
    Adbc81925,
    /// 使用箇所: ruby27 547 (string1), ruby30 558 (string1), ruby31 570 (string1), ruby32 570 (string1) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.character(val[0])
    /// ```
    Adceaf1cf,
    /// 使用箇所: ruby27 100 (mlhs_node), ruby27 102 (mlhs_node), ruby30 106 (mlhs_node), ruby30 108 (mlhs_node) and 8 more.
    ///
    /// ```ruby
    /// if (val[1][0] == :anddot)
    /// diagnostic :error, :csend_in_lhs_of_masgn, nil, val[1]
    /// end
    /// result = @builder.attr_asgn(val[0], val[1], val[2])
    /// ```
    Ade28d5df,
    /// 使用箇所: ruby27 483 (p_args_tail).
    ///
    /// ```ruby
    /// match_rest = @builder.match_rest(val[0], val[1])
    /// result = [ match_rest ]
    /// ```
    Ade2a1b90,
    /// 使用箇所: ruby27 88 (mlhs_basic), ruby30 94 (mlhs_basic), ruby31 98 (mlhs_basic), ruby32 98 (mlhs_basic) and 2 more.
    ///
    /// ```ruby
    /// result = [ @builder.splat(val[0], val[1]),
    /// *val[3] ]
    /// ```
    Ade3a9256,
    /// 使用箇所: ruby30 447 (@27), ruby31 456 (@30), ruby32 458 (@30), ruby33 458 (@24) and 1 more.
    ///
    /// ```ruby
    /// @pattern_variables.pop
    /// @pattern_hash_keys.pop
    /// @context.in_kwarg = val[1]
    /// ```
    Ade93565b,
    /// 使用箇所: ruby27 674 (f_block_arg), ruby30 687 (f_block_arg), ruby31 701 (f_block_arg), ruby32 699 (f_block_arg) and 2 more.
    ///
    /// ```ruby
    /// @static_env.declare val[1][0]
    /// result = @builder.blockarg(val[0], val[1])
    /// ```
    Adf085caa,
    /// 使用箇所: ruby30 498 (p_rest), ruby31 508 (p_rest), ruby32 508 (p_rest), ruby33 508 (p_rest) and 1 more.
    ///
    /// ```ruby
    /// result = @builder.match_rest(val[0])
    /// ```
    Adf52b570,
    /// 使用箇所: ruby31 679 (f_block_kw), ruby32 677 (f_block_kw), ruby33 678 (f_block_kw), ruby34 678 (f_block_kw).
    ///
    /// ```ruby
    /// @context.in_argdef = true
    /// result = @builder.kwarg(val[0])
    /// ```
    Ae0e51d1a,
    /// 使用箇所: ruby27 21 (stmt), ruby30 21 (stmt), ruby31 21 (stmt), ruby32 21 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.alias(val[0],
    /// @builder.gvar(val[1]),
    /// @builder.gvar(val[2]))
    /// ```
    Ae0eac606,
    /// 使用箇所: ruby27 54 (expr).
    ///
    /// ```ruby
    /// @pattern_variables.pop
    /// @context.in_kwarg = val[2]
    /// if @builder.class.emit_match_pattern
    /// result = @builder.match_pattern(val[0], val[1], val[3])
    /// else
    /// result = @builder.in_match(val[0], val[1], val[3])
    /// end
    /// ```
    Ae166e7bf,
    /// 使用箇所: ruby27 468 (@31), ruby30 479 (@28), ruby31 489 (@31), ruby32 491 (@31) and 2 more.
    ///
    /// ```ruby
    /// @pattern_hash_keys.push
    /// result = @context.in_kwarg
    /// @context.in_kwarg = false
    /// ```
    Ae1d806cc,
    /// 使用箇所: ruby31 67 (@6), ruby32 67 (@6), ruby33 66 (@4), ruby34 66 (@4).
    ///
    /// ```ruby
    /// @lexer.state = :expr_fname
    /// @context.in_argdef = true
    /// ```
    Ae254a9b9,
    /// 使用箇所: ruby27 22 (stmt), ruby30 22 (stmt), ruby31 22 (stmt), ruby32 22 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.alias(val[0],
    /// @builder.gvar(val[1]),
    /// @builder.back_ref(val[2]))
    /// ```
    Ae398a702,
    /// 使用箇所: ruby27 327 (primary), ruby30 336 (primary), ruby31 343 (primary), ruby32 345 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.for(val[0], val[1], val[2], *val[3], val[4], val[5])
    /// ```
    Ae41e2bcc,
    /// 使用箇所: ruby27 531 (opt_rescue), ruby30 542 (opt_rescue), ruby31 554 (opt_rescue), ruby32 554 (opt_rescue) and 2 more.
    ///
    /// ```ruby
    /// assoc_t, exc_var = val[2]
    /// if val[1]
    /// exc_list = @builder.array(nil, val[1], nil)
    /// end
    /// result = [ @builder.rescue_body(val[0],
    /// exc_list, assoc_t, exc_var,
    /// val[3], val[4]),
    /// *val[5] ]
    /// ```
    Ae441ba10,
    /// 使用箇所: ruby31 412 (block_param_def), ruby32 414 (block_param_def), ruby33 412 (block_param_def), ruby34 412 (block_param_def).
    ///
    /// ```ruby
    /// @max_numparam_stack.has_ordinary_params!
    /// @current_arg_stack.set(nil)
    /// @context.in_argdef = false
    /// result = @builder.args(val[0], val[1].concat(val[2]), val[3])
    /// ```
    Ae46717b8,
    /// 使用箇所: ruby31 329 (primary), ruby32 331 (primary).
    ///
    /// ```ruby
    /// @context.in_defined = false
    /// result = @builder.keyword_cmd(:defined?, val[0],
    /// val[2], [ val[4] ], val[5])
    /// ```
    Ae4f24bd2,
    /// 使用箇所: ruby27 90 (mlhs_basic), ruby30 96 (mlhs_basic), ruby31 100 (mlhs_basic), ruby32 100 (mlhs_basic) and 2 more.
    ///
    /// ```ruby
    /// result = [ @builder.splat(val[0]),
    /// *val[2] ]
    /// ```
    Ae50dd124,
    /// 使用箇所: ruby27 80 (mlhs_inner), ruby27 360 (f_marg), ruby27 645 (f_arg_item), ruby30 86 (mlhs_inner) and 14 more.
    ///
    /// ```ruby
    /// result = @builder.multi_lhs(val[0], val[1], val[2])
    /// ```
    Ae66ee726,
    /// 使用箇所: ruby27 103 (mlhs_node), ruby27 112 (lhs), ruby30 109 (mlhs_node), ruby30 118 (lhs) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.assignable(
    /// @builder.const_fetch(val[0], val[1], val[2]))
    /// ```
    Ae6a95782,
    /// 使用箇所: ruby27 378 (block_param), ruby27 622 (f_args), ruby30 386 (block_param), ruby30 635 (f_args) and 8 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// concat(val[2]).
    /// concat(val[4]).
    /// concat(val[6]).
    /// concat(val[7])
    /// ```
    Ae8145a70,
    /// 使用箇所: ruby32 526 (p_any_kwrest), ruby33 526 (p_any_kwrest), ruby34 526 (p_any_kwrest).
    ///
    /// ```ruby
    /// result = [ @builder.match_nil_pattern(val[0][0], val[0][1]) ]
    /// ```
    Ae8e897b4,
    /// 使用箇所: ruby27 216 (arg), ruby27 217 (arg), ruby27 218 (arg), ruby27 219 (arg) and 104 more.
    ///
    /// ```ruby
    /// result = @builder.binary_op(val[0], val[1], val[2])
    /// ```
    Ae9106b4e,
    /// 使用箇所: ruby27 661 (f_kwrest), ruby30 674 (f_kwrest), ruby31 688 (f_kwrest).
    ///
    /// ```ruby
    /// result = [ @builder.kwrestarg(val[0]) ]
    /// ```
    Aea059bc4,
    /// 使用箇所: ruby27 320 (primary), ruby30 329 (primary), ruby31 336 (primary), ruby32 338 (primary) and 2 more.
    ///
    /// ```ruby
    /// else_t, else_ = val[4]
    /// result = @builder.condition(val[0], val[1], val[2],
    /// val[3], else_t,
    /// else_,  val[5])
    /// ```
    Aea41f5d0,
    /// 使用箇所: ruby32 696 (f_rest_arg), ruby33 697 (f_rest_arg), ruby34 697 (f_rest_arg).
    ///
    /// ```ruby
    /// @static_env.declare_anonymous_restarg
    /// result = [ @builder.restarg(val[0]) ]
    /// ```
    Aeb600ee2,
    /// 使用箇所: ruby27 5 (top_stmts), ruby27 14 (stmts), ruby27 93 (mlhs_head), ruby27 95 (mlhs_post) and 157 more.
    ///
    /// ```ruby
    /// result = [ val[0] ]
    /// ```
    Aec2d86e3,
    /// 使用箇所: ruby27 323 (primary), ruby30 332 (primary), ruby31 339 (primary), ruby32 341 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.loop(:until, val[0], *val[1], val[2], val[3])
    /// ```
    Aec5b9a1a,
    /// 使用箇所: ruby27 573 (string_content), ruby30 584 (string_content), ruby31 596 (string_content), ruby32 596 (string_content) and 2 more.
    ///
    /// ```ruby
    /// @lexer.cmdarg.pop
    /// @lexer.cond.pop
    /// result = @builder.begin(val[0], val[2], val[3])
    /// ```
    Aeeba0c0e,
    /// 使用箇所: ruby27 86 (mlhs_basic), ruby30 92 (mlhs_basic), ruby31 96 (mlhs_basic), ruby32 96 (mlhs_basic) and 2 more.
    ///
    /// ```ruby
    /// result = val[0].
    /// push(@builder.splat(val[1])).
    /// concat(val[3])
    /// ```
    Aef2a3ac4,
    /// 使用箇所: ruby27 394 (block_param_def), ruby30 402 (block_param_def).
    ///
    /// ```ruby
    /// @max_numparam_stack.has_ordinary_params!
    /// @current_arg_stack.set(nil)
    /// result = @builder.args(val[0], val[1], val[2])
    /// ```
    Af0965e7c,
    /// 使用箇所: ruby27 652 (f_block_kw), ruby30 665 (f_block_kw).
    ///
    /// ```ruby
    /// result = @builder.kwarg(val[0])
    /// ```
    Af0b75464,
    /// 使用箇所: ruby27 314 (primary), ruby30 324 (primary), ruby31 331 (primary), ruby32 333 (primary) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.not_op(val[0], val[1], nil, val[2])
    /// ```
    Af0caa16d,
    /// 使用箇所: ruby27 33 (stmt), ruby30 33 (stmt), ruby31 33 (stmt), ruby32 33 (stmt) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.assign(val[0], val[1],
    /// @builder.array(nil, val[2], nil))
    /// ```
    Af0ef13f6,
    /// 使用箇所: ruby27 397 (opt_bv_decl), ruby30 405 (opt_bv_decl), ruby31 414 (opt_bv_decl), ruby32 416 (opt_bv_decl) and 2 more.
    ///
    /// ```ruby
    /// result = val[2]
    /// ```
    Af108a4ea,
    /// 使用箇所: ruby27 438 (@28), ruby30 446 (@26), ruby31 57 (@3), ruby31 59 (@4) and 4 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_beg
    /// @lexer.command_start = false
    /// @pattern_variables.push
    /// @pattern_hash_keys.push
    /// result = @context.in_kwarg
    /// @context.in_kwarg = true
    /// ```
    Af1765b94,
    /// 使用箇所: ruby34 64 (def_name).
    ///
    /// ```ruby
    /// local_push
    /// @current_arg_stack.push(nil)
    /// result = [ val[0], @context.dup ]
    /// @context.in_def = true
    /// @context.cant_return = false
    /// ```
    Af1a7660a,
    /// 使用箇所: ruby27 545 (string1), ruby30 556 (string1), ruby31 568 (string1), ruby32 568 (string1) and 2 more.
    ///
    /// ```ruby
    /// string = @builder.string_compose(val[0], val[1], val[2])
    /// result = @builder.dedent_string(string, @lexer.dedent_level)
    /// ```
    Af268de85,
    /// 使用箇所: ruby34 360 (k_return).
    ///
    /// ```ruby
    /// if @context.cant_return && !(context.in_block || context.in_lambda)
    /// diagnostic :error, :invalid_return, nil, val[0]
    /// end
    /// ```
    Af2e1c9dc,
    /// 使用箇所: ruby27 594 (keyword_variable), ruby30 605 (keyword_variable), ruby31 620 (keyword_variable), ruby32 618 (keyword_variable) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.self(val[0])
    /// ```
    Af4175410,
    /// 使用箇所: ruby30 344 (primary).
    ///
    /// ```ruby
    /// def_t, recv, dot_t, (name_t, ctx) = val[0]
    /// result = @builder.def_singleton(def_t, recv, dot_t, name_t, val[1],
    /// val[2], val[3])
    /// local_pop
    /// @context.in_def = ctx.in_def
    /// @current_arg_stack.pop
    /// ```
    Af463df71,
    /// 使用箇所: ruby27 299 (@9), ruby27 301 (@10), ruby30 309 (@11), ruby30 311 (@12) and 6 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_endarg
    /// ```
    Af4ea540c,
    /// 使用箇所: ruby27 580 (ssym), ruby30 591 (ssym), ruby31 603 (ssym), ruby32 603 (ssym) and 2 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_end
    /// result = @builder.symbol(val[0])
    /// ```
    Af533deb8,
    /// 使用箇所: ruby27 6 (top_stmts), ruby27 15 (stmts), ruby27 96 (mlhs_post), ruby27 279 (args) and 74 more.
    ///
    /// ```ruby
    /// result = val[0] << val[2]
    /// ```
    Af55455b1,
    /// 使用箇所: ruby27 644 (f_arg_item), ruby30 657 (f_arg_item), ruby31 671 (f_arg_item), ruby32 669 (f_arg_item) and 2 more.
    ///
    /// ```ruby
    /// @current_arg_stack.set(0)
    /// result = @builder.arg(val[0])
    /// ```
    Af58f6bf7,
    /// 使用箇所: ruby31 385 (@18), ruby32 387 (@18), ruby33 385 (@13), ruby34 385 (@13).
    ///
    /// ```ruby
    /// @context.in_argdef = false
    /// ```
    Af5e5481e,
    /// 使用箇所: ruby27 392 (opt_block_param), ruby30 400 (opt_block_param), ruby30 621 (f_opt_paren_args), ruby31 409 (opt_block_param) and 3 more.
    ///
    /// ```ruby
    /// result = @builder.args(nil, [], nil)
    /// ```
    Af5e87ec6,
    /// 使用箇所: ruby27 253 (aref_args), ruby27 265 (opt_call_args), ruby30 263 (aref_args), ruby30 275 (opt_call_args) and 8 more.
    ///
    /// ```ruby
    /// result = val[0] << @builder.associate(nil, val[2], nil)
    /// ```
    Af6484942,
    /// 使用箇所: ruby30 252 (arg).
    ///
    /// ```ruby
    /// def_t, recv, dot_t, (name_t, ctx) = val[0]
    /// endless_method_name(name_t)
    /// rescue_body = @builder.rescue_body(val[4],
    /// nil, nil, nil,
    /// nil, val[5])
    /// method_body = @builder.begin_body(val[3], [ rescue_body ])
    /// result = @builder.def_endless_singleton(def_t, recv, dot_t, name_t,
    /// val[1], val[2], method_body)
    /// local_pop
    /// @context.in_def = ctx.in_def
    /// @current_arg_stack.pop
    /// ```
    Af6a4a496,
    /// 使用箇所: ruby27 586 (simple_numeric), ruby30 597 (simple_numeric), ruby31 609 (simple_numeric), ruby32 609 (simple_numeric) and 2 more.
    ///
    /// ```ruby
    /// @lexer.state = :expr_end
    /// result = @builder.rational(val[0])
    /// ```
    Af6b173c0,
    /// 使用箇所: ruby30 343 (primary).
    ///
    /// ```ruby
    /// def_t, (name_t, ctx) = val[0]
    /// result = @builder.def_method(def_t, name_t, val[1],
    /// val[2], val[3])
    /// local_pop
    /// @context.in_def = ctx.in_def
    /// @current_arg_stack.pop
    /// ```
    Af6dcd9d2,
    /// 使用箇所: ruby31 351 (primary), ruby32 353 (primary), ruby33 351 (primary), ruby34 351 (primary).
    ///
    /// ```ruby
    /// def_t, recv, dot_t, (name_t, ctx) = val[0]
    /// result = @builder.def_singleton(def_t, recv, dot_t, name_t, val[1],
    /// val[2], val[3])
    /// local_pop
    /// @current_arg_stack.pop
    /// @context.in_def = ctx.in_def
    /// ```
    Af73cab7a,
    /// 使用箇所: ruby31 423 (f_larglist), ruby32 425 (f_larglist), ruby33 423 (f_larglist), ruby34 423 (f_larglist).
    ///
    /// ```ruby
    /// @context.in_argdef = false
    /// if val[0].any?
    /// @max_numparam_stack.has_ordinary_params!
    /// end
    /// result = @builder.args(nil, val[0], nil)
    /// ```
    Af798d7ca,
    /// 使用箇所: ruby27 213 (arg), ruby27 509 (p_value), ruby30 219 (arg), ruby30 521 (p_value) and 8 more.
    ///
    /// ```ruby
    /// result = @builder.range_exclusive(val[0], val[1], nil)
    /// ```
    Af7bda890,
    /// 使用箇所: ruby30 476 (p_expr_basic), ruby31 486 (p_expr_basic), ruby32 488 (p_expr_basic), ruby33 488 (p_expr_basic) and 1 more.
    ///
    /// ```ruby
    /// result = @builder.array_pattern(val[0], val[1], val[2])
    /// ```
    Af8de7941,
    /// 使用箇所: ruby27 1 (@1).
    ///
    /// ```ruby
    /// @current_arg_stack.push(nil)
    /// @max_numparam_stack.push(static: false)
    /// ```
    Af97c9d62,
    /// 使用箇所: ruby27 354 (if_tail), ruby30 360 (if_tail), ruby31 367 (if_tail), ruby32 369 (if_tail) and 2 more.
    ///
    /// ```ruby
    /// else_t, else_ = val[4]
    /// result = [ val[0],
    /// @builder.condition(val[0], val[1], val[2],
    /// val[3], else_t,
    /// else_,  nil),
    /// ]
    /// ```
    Afb02be77,
    /// 使用箇所: ruby27 452 (p_as), ruby30 461 (p_as), ruby31 470 (p_as), ruby32 472 (p_as) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.match_as(val[0], val[1], val[2])
    /// ```
    Afd8f587a,
    /// 使用箇所: ruby27 445 (p_top_expr), ruby30 453 (p_top_expr), ruby31 462 (p_top_expr), ruby32 464 (p_top_expr) and 2 more.
    ///
    /// ```ruby
    /// result = [ val[0], @builder.unless_guard(val[1], val[2]) ]
    /// ```
    Afe2388f0,
    /// 使用箇所: ruby27 481 (p_args_head), ruby30 492 (p_args_head), ruby31 502 (p_args_head), ruby32 502 (p_args_head) and 2 more.
    ///
    /// ```ruby
    /// item = @builder.match_with_trailing_comma(val[0], val[1])
    /// result = [ item ]
    /// ```
    Afe2c559a,
    /// 使用箇所: ruby27 604 (backref), ruby30 615 (backref), ruby31 630 (backref), ruby32 628 (backref) and 2 more.
    ///
    /// ```ruby
    /// result = @builder.nth_ref(val[0])
    /// ```
    Afe654a53,
    /// 使用箇所: ruby27 63 (@5), ruby27 411 (@22), ruby27 426 (@23), ruby27 428 (@24) and 20 more.
    ///
    /// ```ruby
    /// result = @context.dup
    /// @context.in_block = true
    /// ```
    Aff9a11eb,
    /// 使用箇所: ruby27 651 (f_block_kw), ruby30 664 (f_block_kw).
    ///
    /// ```ruby
    /// result = @builder.kwoptarg(val[0], val[1])
    /// ```
    Affab5fdf,
}
