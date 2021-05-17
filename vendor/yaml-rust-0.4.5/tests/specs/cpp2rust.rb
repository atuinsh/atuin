#!/usr/bin/env ruby

TEST_REGEX = /TEST_F\([a-zA-Z0-9_]+,\s+([a-zA-Z0-9_]+)\)/

DISABLED_TESTS = %w(
	test_ex7_10_plain_characters
	test_ex7_17_flow_mapping_separate_values
	test_ex7_21_single_pair_implicit_entries
	test_ex7_2_empty_nodes
	test_ex8_2_block_indentation_header
)

class Context
	attr_accessor :name, :ev, :src
	def initialize
		@name = ""
		@src = ""
		@ev = []
	end
end

class String
	def snakecase
		self
		.gsub(/([A-Z]+)([A-Z][a-z])/, '\1_\2')
		.gsub(/([a-z\d])([A-Z])/, '\1_\2')
		.tr('-', '_')
		.gsub(/\s/, '_')
		.gsub(/__+/, '_')
		.downcase
	end
end

ctx = nil

tests = []
IO.foreach(ARGV[0]) do |line|
	line.strip!
	if ctx
		fail "unexpected TEST_F" if line =~ TEST_REGEX
		if line =~ /^}/
			tests << ctx
			ctx = nil
		end
		if line =~ /^EXPECT_CALL/
			fail 'not end with ;' unless line[-1] == ';'
			v = line.gsub('(', ' ').gsub(')', ' ').split
			ctx.ev << v[2]
		end
	else
		next unless line =~ TEST_REGEX
		name = $1
		next unless name =~ /^(Ex\d+_\d+)/
		str = $1.upcase
		$stderr.puts "found #{name}"
		ctx = Context.new
		ctx.name = "test_#{name.snakecase}"
		ctx.src = str
	end
end

# code gen
tests.each do |t|
	next if t.ev.size == 0
	if DISABLED_TESTS.include? t.name
		puts "#[allow(dead_code)]"
	else
		puts "#[test]"
	end
	puts "fn #{t.name}() {"
	puts "    let mut v = str_to_test_events(#{t.src}).into_iter();"
	t.ev.each do |e|
		puts "    assert_next!(v, TestEvent::#{e});"
	end
	puts "}"
	puts
end

