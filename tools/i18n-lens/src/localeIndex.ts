import * as vscode from 'vscode'
import * as path from 'node:path'
import { isMap, isScalar, parseDocument, type Scalar, type YAMLMap } from 'yaml'

/// 语言标记的形态，如 en、zh-CN、zh_Hant
const LOCALE_CODE = /^[A-Za-z]{2,3}(?:[_-][A-Za-z0-9]{1,8})*$/

export interface Translation {
  locale: string
  text: string
  uri: vscode.Uri
  range: vscode.Range
}

interface FileIndex {
  localeCodes: string[]
  entries: Map<string, Translation[]>
}

/// 遍历时用到的位置换算
type PositionOf = (offset: number) => vscode.Position

export class LocaleIndex {
  private files = new Map<string, FileIndex>()

  clear(): void {
    this.files.clear()
  }

  set(uri: vscode.Uri, text: string): void {
    const parsed = parseFile(uri, text)
    if (parsed) this.files.set(uri.toString(), parsed)
    else this.files.delete(uri.toString())
  }

  remove(uri: vscode.Uri): void {
    this.files.delete(uri.toString())
  }

  /// 所有语言标记的并集，用于判断某个键缺哪些译文
  get localeCodes(): string[] {
    const codes = new Set<string>()
    for (const file of this.files.values()) {
      for (const code of file.localeCodes) codes.add(code)
    }
    return [...codes].sort((a, b) => a.localeCompare(b))
  }

  /// 键在所有语言文件中的全部条目
  all(key: string): Translation[] {
    const result: Translation[] = []
    for (const file of this.files.values()) {
      const hit = file.entries.get(key)
      if (hit) result.push(...hit)
    }
    return result
  }

  /// 键按语言去重后的条目，先出现的文件优先
  lookup(key: string): Map<string, Translation> | undefined {
    const translations = this.all(key)
    if (!translations.length) return undefined
    const byLocale = new Map<string, Translation>()
    for (const translation of translations) {
      if (!byLocale.has(translation.locale)) byLocale.set(translation.locale, translation)
    }
    return byLocale
  }
}

function parseFile(uri: vscode.Uri, text: string): FileIndex | undefined {
  const document = parseDocument(text, { uniqueKeys: false })
  const contents = document.contents
  if (!contents) return undefined

  const entries = new Map<string, Translation[]>()
  const localeCodes = new Set<string>()
  const positionOf = createPositionOf(text)

  const record = (key: string, items: Translation[]) => {
    if (!key || !items.length) return
    const existing = entries.get(key)
    if (existing) existing.push(...items)
    else entries.set(key, items)
  }

  if (readVersion(contents) >= 2) {
    // 语言位于叶子：{ key: { en: ..., zh-CN: ... } }
    const walk = (node: unknown, keyPath: string[]) => {
      if (!isMap(node)) return
      if (isLocaleGroup(node)) {
        const items: Translation[] = []
        for (const pair of node.items) {
          const locale = scalarKey(pair.key)
          if (!locale || !isScalar(pair.value)) continue
          localeCodes.add(locale)
          items.push({
            locale,
            text: scalarText(pair.value),
            uri,
            range: rangeOf(pair.value, positionOf),
          })
        }
        record(keyPath.join('.'), items)
        return
      }
      for (const pair of node.items) {
        const name = scalarKey(pair.key)
        if (!name || name.startsWith('_')) continue
        walk(pair.value, [...keyPath, name])
      }
    }
    walk(contents, [])
  } else {
    // 语言来自文件名：locales/en.yml
    const locale = localeFromPath(uri)
    if (!locale) return undefined
    localeCodes.add(locale)
    const walk = (node: unknown, keyPath: string[]) => {
      if (isMap(node)) {
        for (const pair of node.items) {
          const name = scalarKey(pair.key)
          if (!name || name.startsWith('_')) continue
          walk(pair.value, [...keyPath, name])
        }
        return
      }
      if (!isScalar(node) || !keyPath.length) return
      record(keyPath.join('.'), [
        { locale, text: scalarText(node), uri, range: rangeOf(node, positionOf) },
      ])
    }
    walk(contents, [])
  }

  return { localeCodes: [...localeCodes], entries }
}

/// 该映射的每一项都是语言标记且值是标量时，视为一组译文
function isLocaleGroup(node: YAMLMap): boolean {
  if (!node.items.length) return false
  for (const pair of node.items) {
    const name = scalarKey(pair.key)
    if (!name || !LOCALE_CODE.test(name)) return false
    if (!isScalar(pair.value)) return false
  }
  return true
}

function readVersion(contents: unknown): number {
  if (!isMap(contents)) return 1
  for (const pair of contents.items) {
    if (scalarKey(pair.key) !== '_version') continue
    if (!isScalar(pair.value)) continue
    const version = Number(pair.value.value)
    if (Number.isFinite(version)) return version
  }
  return 1
}

function localeFromPath(uri: vscode.Uri): string | undefined {
  const stem = path.basename(uri.fsPath, path.extname(uri.fsPath))
  if (LOCALE_CODE.test(stem)) return stem
  const parent = path.basename(path.dirname(uri.fsPath))
  if (LOCALE_CODE.test(parent)) return parent
  return undefined
}

function scalarKey(key: unknown): string | undefined {
  if (!isScalar(key)) return undefined
  return key.value == null ? undefined : String(key.value)
}

function scalarText(node: Scalar): string {
  return node.value == null ? '' : String(node.value)
}

function rangeOf(node: unknown, positionOf: PositionOf): vscode.Range {
  const offset = (node as { range?: [number, number, number] | null }).range?.[0] ?? 0
  const point = positionOf(offset)
  return new vscode.Range(point, point)
}

/// 每行起始偏移，把 YAML 节点偏移换算成文档位置
function createPositionOf(text: string): PositionOf {
  const lineStarts: number[] = [0]
  for (let index = 0; index < text.length; index++) {
    if (text[index] === '\n') lineStarts.push(index + 1)
  }
  return (offset: number) => {
    let low = 0
    let high = lineStarts.length - 1
    while (low < high) {
      const mid = (low + high + 1) >> 1
      if (lineStarts[mid] <= offset) low = mid
      else high = mid - 1
    }
    const column = Math.max(0, offset - lineStarts[low])
    return new vscode.Position(low, column)
  }
}
