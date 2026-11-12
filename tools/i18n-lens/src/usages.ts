import * as vscode from 'vscode'

const CALL = /(?<![A-Za-z0-9_])t!\s*\(\s*/g

/// 只处理形如 foo.bar 的键，跳过 t! 直接写整句文案的情况
const KEY_SHAPE = /^[A-Za-z0-9_][A-Za-z0-9_.\-]*$/

export interface Usage {
  key: string
  range: vscode.Range
}

interface Literal {
  value: string
  start: number
  end: number
}

export function findUsages(
  text: string,
  positionAt: (offset: number) => vscode.Position,
  limit = 5000,
): Usage[] {
  const usages: Usage[] = []
  CALL.lastIndex = 0
  let match: RegExpExecArray | null
  while (usages.length < limit && (match = CALL.exec(text))) {
    const literal = readLiteral(text, match.index + match[0].length)
    if (!literal || !KEY_SHAPE.test(literal.value)) continue
    usages.push({
      key: literal.value,
      range: new vscode.Range(positionAt(literal.start), positionAt(literal.end)),
    })
  }
  return usages
}

/// 读取 t!( 之后紧跟的字符串字面量，支持 "..."、'...' 与 r#"..."#
function readLiteral(text: string, start: number): Literal | undefined {
  const head = text[start]
  if (head === 'r') {
    let cursor = start + 1
    while (text[cursor] === '#') cursor++
    if (text[cursor] !== '"') return undefined
    const hashes = cursor - start - 1
    const contentStart = cursor + 1
    const closing = `"${'#'.repeat(hashes)}`
    const end = text.indexOf(closing, contentStart)
    if (end < 0) return undefined
    return { value: text.slice(contentStart, end), start: contentStart, end }
  }
  if (head !== '"' && head !== "'") return undefined
  const contentStart = start + 1
  let cursor = contentStart
  while (cursor < text.length) {
    const char = text[cursor]
    if (char === '\\') {
      cursor += 2
      continue
    }
    if (char === '\n') return undefined
    if (char === head) {
      return { value: text.slice(contentStart, cursor), start: contentStart, end: cursor }
    }
    cursor++
  }
  return undefined
}
