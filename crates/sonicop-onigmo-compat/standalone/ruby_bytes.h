/* Regexp.new の前処理で使う String/Encoding API だけを、VM のない byte buffer に対応させる。 */
#ifndef SONICOP_RUBY_BYTES_H
#define SONICOP_RUBY_BYTES_H
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _MSC_VER
#include <malloc.h>
#define alloca _alloca
#else
#include <alloca.h>
#endif
#include "regenc.h"
extern const OnigEncodingType OnigEncodingUS_ASCII;
typedef const OnigEncodingType rb_encoding;
typedef ptrdiff_t rb_len_t;
typedef char onig_errmsg_buffer[ONIG_MAX_ERROR_MESSAGE_LEN];
typedef struct ruby_bytes { unsigned char *bytes; size_t length, capacity; rb_encoding *encoding; struct ruby_bytes *next; } *VALUE;
/* Rust の専用 worker だけが、この呼出し単位の arena と native globals を触る。 */
static VALUE arena;
static int allocation_failed;
#define Qnil NULL
#define NIL_P(value) ((value) == NULL)
#define ALLOCA_N(type, count) ((type *)alloca(sizeof(type) * (count)))
#define RSTRING_PTR(value) ((char *)(value)->bytes)
#define RSTRING_LEN(value) ((value)->length)
#define ISSPACE(c) ((c) == ' ' || ((unsigned char)(c) >= 9 && (unsigned char)(c) <= 13))
#define ISPRINT(c) ((unsigned int)(c) >= 32 && (unsigned int)(c) <= 126)
#define MBCLEN_CHARFOUND_P(n) ONIGENC_MBCLEN_CHARFOUND_P(n)
#define MBCLEN_CHARFOUND_LEN(n) ONIGENC_MBCLEN_CHARFOUND_LEN(n)
#define MBCLEN_NEEDMORE_P(n) ONIGENC_MBCLEN_NEEDMORE_P(n)
#define MBCLEN_INVALID_P(n) ONIGENC_MBCLEN_INVALID_P(n)
#define rb_enc_precise_mbclen(p,e,enc) ONIGENC_PRECISE_MBC_ENC_LEN(enc,(const UChar *)(p),(const UChar *)(e))
#define mbclen(p,e,enc) onigenc_mbclen((const UChar *)(p),(const UChar *)(e),enc)
#define rb_enc_mbmaxlen(enc) ONIGENC_MBC_MAXLEN(enc)
#define rb_enc_asciicompat(enc) (ONIGENC_MBC_MINLEN(enc) == 1)
#define rb_enc_unicode_p(enc) ((enc) == ONIG_ENCODING_UTF_8)
#define rb_utf8_encoding() ONIG_ENCODING_UTF_8
#define rb_is_usascii_enc(enc) ((enc) == &OnigEncodingUS_ASCII)
#define rb_enc_associate(value, enc) do { if (value) (value)->encoding = (enc); } while (0)
#define rb_enc_mbc_to_codepoint(p,e,enc) ONIGENC_MBC_TO_CODE(enc,(const UChar *)(p),(const UChar *)(e))
#define rb_enc_isprint(c,enc) ONIGENC_IS_CODE_PRINT(enc,c)
#define rb_enc_isspace(c,enc) ONIGENC_IS_CODE_SPACE(enc,c)
#define rb_uv_to_utf8(buf,c) ONIGENC_CODE_TO_MBC(ONIG_ENCODING_UTF_8,c,(UChar *)(buf))
#define ENC_CODERANGE_UNKNOWN 0
#define ENC_CODERANGE_7BIT 1
#define ENC_CODERANGE_VALID 2
#define ENC_CODERANGE_BROKEN 3
#define ENC_CODERANGE_CLEAN_P(n) ((n) == ENC_CODERANGE_7BIT || (n) == ENC_CODERANGE_VALID)
static void errcpy(char *error, const char *message) { snprintf(error, ONIG_MAX_ERROR_MESSAGE_LEN, "%s", message); }
static VALUE rb_str_buf_new(size_t initial) {
    VALUE value = calloc(1, sizeof(*value));
    if (!value) { allocation_failed = 1; return NULL; }
    value->encoding = ONIG_ENCODING_ASCII;value->next = arena;arena = value;
    (void)initial;return value;
}
static void rb_str_buf_cat(VALUE value, const char *bytes, size_t length) {
    if (allocation_failed || !value || !length) return;
    if (length > SIZE_MAX - value->length - 1) { allocation_failed = 1; return; }
    size_t needed = value->length + length + 1;
    if (needed > value->capacity) {
        size_t capacity = needed > SIZE_MAX / 2 ? needed : needed * 2;
        void *resized = realloc(value->bytes, capacity);
        if (!resized) { allocation_failed = 1; return; }
        value->bytes = resized;value->capacity = capacity;
    }
    memcpy(value->bytes + value->length, bytes, length);value->length += length;value->bytes[value->length] = 0;
}
static void free_arena(void) { while (arena) { VALUE next = arena->next; free(arena->bytes);free(arena);arena = next; } }
static unsigned long scan_radix(const char *bytes, size_t length, size_t *used, unsigned radix) {
    unsigned long value = 0;size_t i = 0;
    for (; i < length; i++) {
        unsigned char c = bytes[i];unsigned n;
        if (c >= '0' && c <= '9') n = c - '0';
        else if (c >= 'a' && c <= 'f') n = c - 'a' + 10;
        else if (c >= 'A' && c <= 'F') n = c - 'A' + 10;
        else break;
        if (n >= radix) break;
        value = value * radix + n;
    }
    *used = i;return value;
}
#define scan_hex(p,n,len) scan_radix(p,n,len,16)
#define ruby_scan_hex(p,n,len) scan_radix(p,n,len,16)
#define scan_oct(p,n,len) scan_radix(p,n,len,8)
#define ruby_scan_oct(p,n,len) scan_radix(p,n,len,8)
static int rb_enc_ascget(const char *p, const char *end, int *length, rb_encoding *enc) {
    (void)end;(void)enc;*length = 1;return (unsigned char)*p < 128 ? (unsigned char)*p : -1;
}
static void rb_str_coderange_scan_restartable(const char *p, const char *end, rb_encoding *enc, int *range) {
    int result = ENC_CODERANGE_7BIT;
    while (p < end) {
        int n = rb_enc_precise_mbclen(p,end,enc);
        if (!MBCLEN_CHARFOUND_P(n)) { *range = ENC_CODERANGE_BROKEN;return; }
        if ((unsigned char)*p >= 128) result = ENC_CODERANGE_VALID;
        p += MBCLEN_CHARFOUND_LEN(n);
    }
    *range = result;
}
#define CHAR_ESC_LEN 13
#include "ruby_escaped_char.inc"
#include "ruby_regexp_display.inc"
#include "ruby_preprocess.inc"
#endif
