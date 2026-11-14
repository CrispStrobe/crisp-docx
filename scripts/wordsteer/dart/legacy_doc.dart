// Headless driver for CrispCloud's legacy `.doc` parser (legacy_doc_parser).
//
// Compiled by `wordsteer.py setup` against the package config of a local
// CrispCloud checkout, so it exercises exactly the parser the app ships:
//
//   dart compile exe --packages=<CrispCloud>/packages/quds_office_engine/\
//     .dart_tool/package_config.json legacy_doc.dart -o legacy_doc
//
// Usage:
//   legacy_doc convert INPUT.doc OUTPUT.docx [--recover] -> JSON report
//     (--recover: degrade auxiliary damage, as CrispCloud does when a person
//      opens a file; needs a parser with `recover:` support)
//   legacy_doc inspect INPUT.doc               -> JSON summary on stdout
//
// The report carries the converter's own limitations/preserved counts so the
// audit can tell "declared limitation" from "silent loss".
import 'dart:convert';
import 'dart:io';

import 'package:legacy_doc_parser/legacy_doc_parser.dart';

void _emit(Map<String, Object?> value) =>
    stdout.writeln(const JsonEncoder().convert(value));

Future<void> main(List<String> args) async {
  if (args.isEmpty ||
      !(args[0] == 'convert' &&
              (args.length == 3 || args.length == 4 && args[3] == '--recover') ||
          args[0] == 'inspect' && args.length == 2)) {
    stderr.writeln('usage: legacy_doc convert IN.doc OUT.docx | inspect IN.doc');
    exitCode = 64;
    return;
  }
  final watch = Stopwatch()..start();
  final bytes = File(args[1]).readAsBytesSync();
  try {
    if (args[0] == 'inspect') {
      final doc = parseLegacyWordDocument(bytes);
      _emit({
        'ok': true,
        'ms': watch.elapsedMilliseconds,
        'chars': doc.plainText.length,
        'warnings': [for (final w in doc.warnings) w.toString()],
      });
      return;
    }
    final result = args.length == 4
        ? Function.apply(convertLegacyWord, [bytes], {#recover: true})
              as LegacyWordStructuredConversion
        : convertLegacyWord(bytes);
    File(args[2]).writeAsBytesSync(result.bytes);
    _emit({
      'ok': true,
      'ms': watch.elapsedMilliseconds,
      'limitations': [for (final l in result.limitations) l.name],
      'preserved': result.preservedCounts,
    });
  } on Object catch (error) {
    // Typed failure class first (stable for aggregation), message second.
    _emit({
      'ok': false,
      'ms': watch.elapsedMilliseconds,
      'errorType': error.runtimeType.toString(),
      'error': error.toString().split('\n').first,
    });
    exitCode = 2;
  }
}
