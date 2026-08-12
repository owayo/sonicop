//! Ruby's exception hierarchy, which `Lint/ShadowedException` reads through `Kernel.const_get`.
//!
//! Upstream resolves each rescued name against the constants of the process RuboCop itself runs
//! in, and compares the classes with `Module#<=>`. What that process holds beyond Ruby's own
//! classes depends on which gems happened to be loaded, so only the classes Ruby defines are
//! listed here: a name this table does not know answers `nil`, exactly as an unresolvable constant
//! does upstream, and a `nil` never makes a group unsorted or multi-levelled.

/// Every class Ruby defines under `Exception`, paired with its superclass. `Object` and
/// `BasicObject` are included so that a `rescue Object` compares the way upstream's does.
const HIERARCHY: &[(&str, &str)] = &[
    ("ArgumentError", "StandardError"),
    ("BasicObject", ""),
    ("ClosedQueueError", "StopIteration"),
    ("EOFError", "IOError"),
    ("Encoding::CompatibilityError", "EncodingError"),
    ("Encoding::ConverterNotFoundError", "EncodingError"),
    ("Encoding::InvalidByteSequenceError", "EncodingError"),
    ("Encoding::UndefinedConversionError", "EncodingError"),
    ("EncodingError", "StandardError"),
    ("Errno::E2BIG", "SystemCallError"),
    ("Errno::EACCES", "SystemCallError"),
    ("Errno::EADDRINUSE", "SystemCallError"),
    ("Errno::EADDRNOTAVAIL", "SystemCallError"),
    ("Errno::EAFNOSUPPORT", "SystemCallError"),
    ("Errno::EAGAIN", "SystemCallError"),
    ("Errno::EALREADY", "SystemCallError"),
    ("Errno::EAUTH", "SystemCallError"),
    ("Errno::EBADARCH", "SystemCallError"),
    ("Errno::EBADEXEC", "SystemCallError"),
    ("Errno::EBADF", "SystemCallError"),
    ("Errno::EBADMACHO", "SystemCallError"),
    ("Errno::EBADMSG", "SystemCallError"),
    ("Errno::EBADRPC", "SystemCallError"),
    ("Errno::EBUSY", "SystemCallError"),
    ("Errno::ECANCELED", "SystemCallError"),
    ("Errno::ECHILD", "SystemCallError"),
    ("Errno::ECONNABORTED", "SystemCallError"),
    ("Errno::ECONNREFUSED", "SystemCallError"),
    ("Errno::ECONNRESET", "SystemCallError"),
    ("Errno::EDEADLK", "SystemCallError"),
    ("Errno::EDESTADDRREQ", "SystemCallError"),
    ("Errno::EDEVERR", "SystemCallError"),
    ("Errno::EDOM", "SystemCallError"),
    ("Errno::EDQUOT", "SystemCallError"),
    ("Errno::EEXIST", "SystemCallError"),
    ("Errno::EFAULT", "SystemCallError"),
    ("Errno::EFBIG", "SystemCallError"),
    ("Errno::EFTYPE", "SystemCallError"),
    ("Errno::EHOSTDOWN", "SystemCallError"),
    ("Errno::EHOSTUNREACH", "SystemCallError"),
    ("Errno::EIDRM", "SystemCallError"),
    ("Errno::EILSEQ", "SystemCallError"),
    ("Errno::EINPROGRESS", "SystemCallError"),
    ("Errno::EINTR", "SystemCallError"),
    ("Errno::EINVAL", "SystemCallError"),
    ("Errno::EIO", "SystemCallError"),
    ("Errno::EISCONN", "SystemCallError"),
    ("Errno::EISDIR", "SystemCallError"),
    ("Errno::ELOOP", "SystemCallError"),
    ("Errno::EMFILE", "SystemCallError"),
    ("Errno::EMLINK", "SystemCallError"),
    ("Errno::EMSGSIZE", "SystemCallError"),
    ("Errno::EMULTIHOP", "SystemCallError"),
    ("Errno::ENAMETOOLONG", "SystemCallError"),
    ("Errno::ENEEDAUTH", "SystemCallError"),
    ("Errno::ENETDOWN", "SystemCallError"),
    ("Errno::ENETRESET", "SystemCallError"),
    ("Errno::ENETUNREACH", "SystemCallError"),
    ("Errno::ENFILE", "SystemCallError"),
    ("Errno::ENOATTR", "SystemCallError"),
    ("Errno::ENOBUFS", "SystemCallError"),
    ("Errno::ENODATA", "SystemCallError"),
    ("Errno::ENODEV", "SystemCallError"),
    ("Errno::ENOENT", "SystemCallError"),
    ("Errno::ENOEXEC", "SystemCallError"),
    ("Errno::ENOLCK", "SystemCallError"),
    ("Errno::ENOLINK", "SystemCallError"),
    ("Errno::ENOMEM", "SystemCallError"),
    ("Errno::ENOMSG", "SystemCallError"),
    ("Errno::ENOPOLICY", "SystemCallError"),
    ("Errno::ENOPROTOOPT", "SystemCallError"),
    ("Errno::ENOSPC", "SystemCallError"),
    ("Errno::ENOSR", "SystemCallError"),
    ("Errno::ENOSTR", "SystemCallError"),
    ("Errno::ENOSYS", "SystemCallError"),
    ("Errno::ENOTBLK", "SystemCallError"),
    ("Errno::ENOTCAPABLE", "SystemCallError"),
    ("Errno::ENOTCONN", "SystemCallError"),
    ("Errno::ENOTDIR", "SystemCallError"),
    ("Errno::ENOTEMPTY", "SystemCallError"),
    ("Errno::ENOTRECOVERABLE", "SystemCallError"),
    ("Errno::ENOTSOCK", "SystemCallError"),
    ("Errno::ENOTSUP", "SystemCallError"),
    ("Errno::ENOTTY", "SystemCallError"),
    ("Errno::ENXIO", "SystemCallError"),
    ("Errno::EOPNOTSUPP", "SystemCallError"),
    ("Errno::EOVERFLOW", "SystemCallError"),
    ("Errno::EOWNERDEAD", "SystemCallError"),
    ("Errno::EPERM", "SystemCallError"),
    ("Errno::EPFNOSUPPORT", "SystemCallError"),
    ("Errno::EPIPE", "SystemCallError"),
    ("Errno::EPROCLIM", "SystemCallError"),
    ("Errno::EPROCUNAVAIL", "SystemCallError"),
    ("Errno::EPROGMISMATCH", "SystemCallError"),
    ("Errno::EPROGUNAVAIL", "SystemCallError"),
    ("Errno::EPROTO", "SystemCallError"),
    ("Errno::EPROTONOSUPPORT", "SystemCallError"),
    ("Errno::EPROTOTYPE", "SystemCallError"),
    ("Errno::EPWROFF", "SystemCallError"),
    ("Errno::EQFULL", "SystemCallError"),
    ("Errno::ERANGE", "SystemCallError"),
    ("Errno::EREMOTE", "SystemCallError"),
    ("Errno::EROFS", "SystemCallError"),
    ("Errno::ERPCMISMATCH", "SystemCallError"),
    ("Errno::ESHLIBVERS", "SystemCallError"),
    ("Errno::ESHUTDOWN", "SystemCallError"),
    ("Errno::ESOCKTNOSUPPORT", "SystemCallError"),
    ("Errno::ESPIPE", "SystemCallError"),
    ("Errno::ESRCH", "SystemCallError"),
    ("Errno::ESTALE", "SystemCallError"),
    ("Errno::ETIME", "SystemCallError"),
    ("Errno::ETIMEDOUT", "SystemCallError"),
    ("Errno::ETOOMANYREFS", "SystemCallError"),
    ("Errno::ETXTBSY", "SystemCallError"),
    ("Errno::EUSERS", "SystemCallError"),
    ("Errno::EXDEV", "SystemCallError"),
    ("Errno::NOERROR", "SystemCallError"),
    ("Exception", "Object"),
    ("FiberError", "StandardError"),
    ("FloatDomainError", "RangeError"),
    ("FrozenError", "RuntimeError"),
    ("Gem::CommandLineError", "Gem::Exception"),
    ("Gem::ConflictError", "Gem::LoadError"),
    ("Gem::DependencyError", "Gem::Exception"),
    ("Gem::DependencyRemovalException", "Gem::Exception"),
    ("Gem::DependencyResolutionError", "Gem::DependencyError"),
    ("Gem::DocumentError", "Gem::Exception"),
    ("Gem::EndOfYAMLException", "Gem::Exception"),
    ("Gem::Exception", "RuntimeError"),
    ("Gem::FilePermissionError", "Gem::Exception"),
    ("Gem::FormatException", "Gem::Exception"),
    ("Gem::GemNotFoundException", "Gem::Exception"),
    ("Gem::GemNotInHomeException", "Gem::Exception"),
    ("Gem::ImpossibleDependenciesError", "Gem::Exception"),
    ("Gem::InstallError", "Gem::Exception"),
    ("Gem::InvalidSpecificationException", "Gem::Exception"),
    ("Gem::LoadError", "LoadError"),
    ("Gem::MissingSpecError", "Gem::LoadError"),
    ("Gem::MissingSpecVersionError", "Gem::MissingSpecError"),
    ("Gem::OperationNotSupportedError", "Gem::Exception"),
    ("Gem::RemoteError", "Gem::Exception"),
    ("Gem::RemoteInstallationCancelled", "Gem::Exception"),
    ("Gem::RemoteInstallationSkipped", "Gem::Exception"),
    ("Gem::RemoteSourceException", "Gem::Exception"),
    ("Gem::Requirement::BadRequirementError", "ArgumentError"),
    ("Gem::RubyVersionMismatch", "Gem::Exception"),
    ("Gem::RuntimeRequirementNotMetError", "Gem::InstallError"),
    (
        "Gem::SpecificGemNotFoundException",
        "Gem::GemNotFoundException",
    ),
    ("Gem::SystemExitException", "SystemExit"),
    ("Gem::UninstallError", "Gem::Exception"),
    ("Gem::UnknownCommandError", "Gem::Exception"),
    ("Gem::UnsatisfiableDependencyError", "Gem::DependencyError"),
    ("Gem::VerificationError", "Gem::Exception"),
    ("Gem::WebauthnVerificationError", "Gem::Exception"),
    ("IO::Buffer::AccessError", "RuntimeError"),
    ("IO::Buffer::AllocationError", "RuntimeError"),
    ("IO::Buffer::InvalidatedError", "RuntimeError"),
    ("IO::Buffer::LockedError", "RuntimeError"),
    ("IO::Buffer::MaskError", "ArgumentError"),
    ("IO::EAGAINWaitReadable", "Errno::EAGAIN"),
    ("IO::EAGAINWaitWritable", "Errno::EAGAIN"),
    ("IO::EINPROGRESSWaitReadable", "Errno::EINPROGRESS"),
    ("IO::EINPROGRESSWaitWritable", "Errno::EINPROGRESS"),
    ("IO::TimeoutError", "IOError"),
    ("IOError", "StandardError"),
    ("IndexError", "StandardError"),
    ("Interrupt", "SignalException"),
    ("KeyError", "IndexError"),
    ("LoadError", "ScriptError"),
    ("LocalJumpError", "StandardError"),
    ("Math::DomainError", "StandardError"),
    ("NameError", "StandardError"),
    ("NoMatchingPatternError", "StandardError"),
    ("NoMatchingPatternKeyError", "NoMatchingPatternError"),
    ("NoMemoryError", "Exception"),
    ("NoMethodError", "NameError"),
    ("NotImplementedError", "ScriptError"),
    ("Object", "BasicObject"),
    ("Ractor::ClosedError", "StopIteration"),
    ("Ractor::Error", "RuntimeError"),
    ("Ractor::IsolationError", "Ractor::Error"),
    ("Ractor::MovedError", "Ractor::Error"),
    ("Ractor::RemoteError", "Ractor::Error"),
    ("Ractor::UnsafeError", "Ractor::Error"),
    ("RangeError", "StandardError"),
    ("Regexp::TimeoutError", "RegexpError"),
    ("RegexpError", "StandardError"),
    ("RuntimeError", "StandardError"),
    ("ScriptError", "Exception"),
    ("SecurityError", "Exception"),
    ("SignalException", "Exception"),
    ("StandardError", "Exception"),
    ("StopIteration", "IndexError"),
    ("SyntaxError", "ScriptError"),
    ("SystemCallError", "StandardError"),
    ("SystemExit", "Exception"),
    ("SystemStackError", "Exception"),
    ("ThreadError", "StandardError"),
    ("TypeError", "StandardError"),
    ("UncaughtThrowError", "ArgumentError"),
    ("ZeroDivisionError", "StandardError"),
];

/// `Kernel.const_get(source)`: the class a rescued name resolves to, as an index into the table.
///
/// The name is the source as written, which is why `::StandardError` resolves to nothing --
/// `const_get` refuses a leading `::`.
pub(super) fn resolve(name: &str) -> Option<usize> {
    HIERARCHY.iter().position(|(known, _)| *known == name)
}

/// `Module#<=>`: `Less` when the first is a descendant of the second, `Greater` when it is the
/// ancestor, `Same` when they are one class, and nothing when they are unrelated.
pub(super) fn compare(left: usize, right: usize) -> Option<std::cmp::Ordering> {
    if left == right {
        return Some(std::cmp::Ordering::Equal);
    }
    if is_descendant(left, right) {
        return Some(std::cmp::Ordering::Less);
    }
    if is_descendant(right, left) {
        return Some(std::cmp::Ordering::Greater);
    }
    None
}

/// Whether `descendant` inherits from `ancestor`, not counting the class itself.
fn is_descendant(descendant: usize, ancestor: usize) -> bool {
    let mut current = HIERARCHY[descendant].1;
    while !current.is_empty() {
        let Some(index) = resolve(current) else {
            return false;
        };
        if index == ancestor {
            return true;
        }
        current = HIERARCHY[index].1;
    }
    false
}

/// Whether the class is one of the `Errno` classes, which upstream tells apart by their
/// `ancestors[1]` being `SystemCallError`.
pub(super) fn is_system_call_error(index: usize) -> bool {
    HIERARCHY[index].1 == "SystemCallError"
}

/// Whether the class is `Exception` itself.
pub(super) fn is_exception(index: usize) -> bool {
    HIERARCHY[index].0 == "Exception"
}
