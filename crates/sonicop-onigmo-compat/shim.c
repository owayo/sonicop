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
int sonicop_onigmo_check(const unsigned char *pattern, size_t length, int encoding,
                        int extended, unsigned char *error, size_t capacity) {
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
        if (!result) onig_free(regex);
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
