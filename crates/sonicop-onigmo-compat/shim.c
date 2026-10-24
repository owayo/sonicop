#include "rename.h"
#include <stdint.h>
#include "ruby_bytes.h"
#include <string.h>
extern const OnigEncodingType OnigEncodingUS_ASCII;
/* LLP64/LP64 の設定を target の C compiler 自身にも照合させる。 */
typedef char sonicop_long_abi[(sizeof(long) == SIZEOF_LONG) ? 1 : -1];
typedef char sonicop_pointer_abi[(sizeof(void *) == SIZEOF_VOIDP) ? 1 : -1];

void sonicop_onigmo_init(void) {
    onig_init();
    onig_set_warn_func(ONIG_NULL_WARN);
    onig_set_verb_warn_func(ONIG_NULL_WARN);
}
int sonicop_onigmo_validate(const unsigned char *pattern, size_t length, int encoding,
                           int extended, unsigned char *error, size_t capacity) {
    OnigEncoding enc;
    switch (encoding) {
      case 1: enc = ONIG_ENCODING_ASCII; break;
      case 2: enc = (&OnigEncodingUS_ASCII); break;
      case 3: enc = ONIG_ENCODING_EUC_JP; break;
      case 4: enc = ONIG_ENCODING_WINDOWS_31J; break;
      default: enc = ONIG_ENCODING_UTF_8; break;
    }
    OnigRegex regex = NULL;
    OnigErrorInfo info;
    int result = onig_new(&regex, pattern, pattern + length,
                         extended ? ONIG_OPTION_EXTEND : ONIG_OPTION_NONE,
                         enc, ONIG_SYNTAX_RUBY, &info);
    if (result == 0) { onig_free(regex); }
    else if (capacity) {
        unsigned char message[ONIG_MAX_ERROR_MESSAGE_LEN];
        onig_error_code_to_str(message, result, &info);
        size_t n = strlen((const char *)message);
        if (n >= capacity) n = capacity - 1;
        memcpy(error, message, n);error[n] = 0;
    }
    return result;
}

/* Regexp.new は lexer の復号後にさらに非 ASCII escape と符号化を検査する。 */
typedef void (*sonicop_name_callback)(const unsigned char *, size_t, int, void *);
struct sonicop_name_context { sonicop_name_callback callback; void *context; };
static int sonicop_collect_name(const unsigned char *name, const unsigned char *name_end,
                                int back_num, int *back_refs, OnigRegex regex, void *context) {
    (void)regex;
    if (!name || !name_end || name_end < name || back_num <= 0 || !back_refs)
        return ONIGERR_INVALID_ARGUMENT;
    int first = back_refs[0];
    for (int i = 1; i < back_num; ++i) if (back_refs[i] < first) first = back_refs[i];
    struct sonicop_name_context *receiver = (struct sonicop_name_context *)context;
    receiver->callback(name, (size_t)(name_end - name), first, receiver->context);
    return 0;
}
int sonicop_onigmo_compile(const unsigned char *pattern, size_t length, int encoding,
                          int extended, unsigned char *error, size_t capacity,
                          int *compiled_encoding_out, sonicop_name_callback callback, void *context) {
    OnigEncoding enc;
    switch (encoding) {
      case 1: enc = ONIG_ENCODING_ASCII; break;
      case 2: enc = &OnigEncodingUS_ASCII; break;
      case 3: enc = ONIG_ENCODING_EUC_JP; break;
      case 4: enc = ONIG_ENCODING_WINDOWS_31J; break;
      default: enc = ONIG_ENCODING_UTF_8; break;
    }
    allocation_failed = 0;
    rb_encoding *fixed = NULL;
    onig_errmsg_buffer reason = "";
    VALUE processed = rb_reg_preprocess((const char *)pattern, (const char *)pattern + length,
                                        enc, &fixed, reason, extended ? ONIG_OPTION_EXTEND : 0);
    int result = -1;
    if (processed && !allocation_failed) {
        OnigRegex regex = NULL;OnigErrorInfo info;
        OnigEncoding compiled_encoding = fixed ? fixed : &OnigEncodingUS_ASCII;
        /* 空の byte buffer も、C の pointer 算術に有効な領域を渡す。 */
        const unsigned char empty[] = {0};
        const unsigned char *bytes = processed->bytes ? processed->bytes : empty;
        result = onig_new(&regex, bytes, bytes + processed->length,
                          extended ? ONIG_OPTION_EXTEND : ONIG_OPTION_NONE,
                          compiled_encoding, ONIG_SYNTAX_RUBY, &info);
        if (!result) {
            if (compiled_encoding_out) {
                OnigEncoding actual = onig_get_encoding(regex);
                *compiled_encoding_out = actual == ONIG_ENCODING_UTF_8 ? 0 :
                    actual == ONIG_ENCODING_ASCII ? 1 : actual == &OnigEncodingUS_ASCII ? 2 :
                    actual == ONIG_ENCODING_EUC_JP ? 3 : actual == ONIG_ENCODING_WINDOWS_31J ? 4 : -1;
            }
            if (callback) {
                struct sonicop_name_context receiver = { callback, context };
                result = onig_foreach_name(regex, sonicop_collect_name, &receiver);
                /* 列挙失敗では未設定の OnigErrorInfo をコンパイルエラーとして読まない。 */
                if (result) errcpy(reason, "failed to collect regexp names");
            }
            onig_free(regex);
        }
        else onig_error_code_to_str((unsigned char *)reason, result, &info);
    }
    if (allocation_failed) { errcpy(reason, "failed to allocate memory");result = ONIGERR_MEMORY; }
    if (result && capacity) {
        VALUE display = rb_str_buf_new(0);
        rb_str_buf_cat(display, reason, strlen(reason));rb_str_buf_cat(display, ": /", 3);
        rb_reg_expr_str(display, (const char *)pattern, length, enc, ONIG_ENCODING_UTF_8, '/');
        rb_str_buf_cat(display, extended ? "/x" : "/", extended ? 2 : 1);
        if (!allocation_failed) {
            size_t n = display->length < capacity - 1 ? display->length : capacity - 1;
            memcpy(error, display->bytes, n);error[n] = 0;
        } else error[0] = 0;
    }
    free_arena();return result;
}

int sonicop_onigmo_check(const unsigned char *pattern, size_t length, int encoding,
                        int extended, unsigned char *error, size_t capacity) {
    return sonicop_onigmo_compile(pattern, length, encoding, extended, error, capacity,
                                  NULL, NULL, NULL);
}
