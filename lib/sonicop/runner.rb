# frozen_string_literal: true

require 'rbconfig'
require 'rubygems'

module Sonicop
  module Runner
    module_function

    def run(arguments = ARGV)
      binary = find_binary
      abort <<~MESSAGE unless binary
        Sonicop's native executable was not found (platform: #{Gem::Platform.local}).
        Reinstall the gem with a Rust toolchain available, or install a platform-specific gem.
      MESSAGE

      begin
        # `[binary, binary]` = exec 形式の強制。`exec(binary, *arguments)` は arguments が空、
        # つまり「カレントディレクトリを見る素の `sonicop`」という最も普通の呼び出しのとき
        # `exec(String)` = commandline 形式に落ちる。この形式は空白で単語分割し、メタ文字が
        # あればまるごと /bin/sh に渡すため、binary のパス次第で ENOENT や exit 127 になる。
        # パスの出所は SONICOP_BINARY と gem の install 先で、どちらもメタ文字を含まない保証がない。
        # argv0 は multi-arg 形式が既定で入れるものと同じ値を明示している。
        exec([binary, binary], *arguments)
      rescue SystemCallError => error
        # 実行できない典型は「platform gem は入ったが実行環境と ABI が違う」ケース
        # (musl 環境に glibc ビルドが入るなど)。素の errno だけでは原因が読めない。
        abort <<~MESSAGE
          Sonicop's native executable could not be started (platform: #{Gem::Platform.local}).
            #{binary}
            #{error.class}: #{error.message}
          The installed platform gem probably does not match this system.
          Reinstall from source with: gem install sonicop --platform ruby
        MESSAGE
      end
    end

    # 環境依存の入力はすべて引数にしてある。既定値は本番の解決経路そのもので、
    # テストからは偽のツリー・偽の spec を渡して候補順を検証する。
    def find_binary(root: gem_root, env: ENV, specification: installed_specification, windows: Gem.win_platform?)
      candidates(root: root, env: env, specification: specification, windows: windows)
        .find { |path| File.file?(path) && File.executable?(path) }
    end

    # 優先順位は「明示指定 > platform gem 同梱 > ソース gem のビルド成果物 > 開発ツリー」。
    def candidates(root: gem_root, env: ENV, specification: installed_specification, windows: Gem.win_platform?)
      executable = windows ? 'sonicop.exe' : 'sonicop'
      installed_executable = windows ? 'sonicop-bin.exe' : 'sonicop-bin'

      paths = []
      paths << env['SONICOP_BINARY'] if env['SONICOP_BINARY']
      paths << File.join(root, 'libexec', executable)
      if specification
        paths << File.join(specification.extension_dir, executable)
        paths << File.join(specification.extension_dir, installed_executable)
        paths << File.join(specification.full_gem_path, 'lib', installed_executable)
      end
      paths << File.join(root, 'target', 'release', executable)
      paths << File.join(root, 'target', 'debug', executable)
      paths
    end

    def gem_root
      File.expand_path('../..', __dir__)
    end

    def installed_specification
      Gem::Specification.find_by_name('sonicop', Sonicop::VERSION)
    rescue Gem::LoadError
      nil # Running from a source checkout.
    end
  end
end
