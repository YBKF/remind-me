import * as vscode from 'vscode'
import { LocaleIndex } from './localeIndex'
import { findUsages, type Usage } from './usages'

interface Settings {
  localesPaths: string[]
  sourceLanguage: string
  displayLanguage: string
  annotate: boolean
  annotateMissing: boolean
  diagnostics: boolean
  ignoreKeyPrefixes: string[]
}

const index = new LocaleIndex()
const diagnostics = vscode.languages.createDiagnosticCollection('i18nLens')
const pending = new Map<string, ReturnType<typeof setTimeout>>()

let settings = readSettings()
let decoration: vscode.TextEditorDecorationType | undefined

export function activate(context: vscode.ExtensionContext): void {
  decoration = vscode.window.createTextEditorDecorationType({
    after: {
      margin: '0 0 0 1.2em',
      color: new vscode.ThemeColor('editorCodeLens.foreground'),
      fontStyle: 'italic',
    },
  })

  context.subscriptions.push(
    decoration,
    diagnostics,
    vscode.commands.registerCommand('i18nLens.reload', () => reload()),
    vscode.languages.registerHoverProvider({ language: 'rust' }, { provideHover }),
    vscode.languages.registerDefinitionProvider({ language: 'rust' }, { provideDefinition }),
    vscode.workspace.onDidOpenTextDocument(onDocumentChanged),
    vscode.workspace.onDidChangeTextDocument(event => onDocumentChanged(event.document)),
    vscode.workspace.onDidCloseTextDocument(document => diagnostics.delete(document.uri)),
    vscode.window.onDidChangeVisibleTextEditors(() => refreshEditors()),
    vscode.window.onDidChangeActiveTextEditor(() => refreshEditors()),
    vscode.workspace.onDidChangeConfiguration(event => {
      if (!event.affectsConfiguration('i18nLens')) return
      settings = readSettings()
      void reload()
    }),
    createLocaleWatcher(),
  )

  void reload()
}

export function deactivate(): void {
  for (const timer of pending.values()) clearTimeout(timer)
  pending.clear()
}

function createLocaleWatcher(): vscode.Disposable {
  const watcher = vscode.workspace.createFileSystemWatcher('**/*.{yml,yaml,json}')
  const disposables = [
    watcher,
    watcher.onDidCreate(uri => void onLocaleFileTouched(uri)),
    watcher.onDidChange(uri => void onLocaleFileTouched(uri)),
    watcher.onDidDelete(uri => onLocaleFileDeleted(uri)),
  ]
  return vscode.Disposable.from(...disposables)
}

async function reload(): Promise<void> {
  index.clear()
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    for (const localePath of settings.localesPaths) {
      const relative = localePath.replace(/^\.?\//, '').replace(/\/+$/, '')
      const pattern = new vscode.RelativePattern(folder, `${relative}/**/*.{yml,yaml,json}`)
      for (const uri of await vscode.workspace.findFiles(pattern)) {
        const text = await readText(uri)
        if (text !== undefined) index.set(uri, text)
      }
    }
  }
  refreshAllDocuments()
}

async function onLocaleFileTouched(uri: vscode.Uri): Promise<void> {
  if (!isLocaleFile(uri)) return
  const text = await readText(uri)
  if (text === undefined) return
  index.set(uri, text)
  refreshAllDocuments()
}

function onLocaleFileDeleted(uri: vscode.Uri): void {
  if (!isLocaleFile(uri)) return
  index.remove(uri)
  refreshAllDocuments()
}

async function readText(uri: vscode.Uri): Promise<string | undefined> {
  try {
    return Buffer.from(await vscode.workspace.fs.readFile(uri)).toString('utf8')
  } catch {
    return undefined
  }
}

function onDocumentChanged(document: vscode.TextDocument): void {
  if (isLocaleFile(document.uri)) {
    index.set(document.uri, document.getText())
    refreshAllDocuments()
    return
  }
  if (document.languageId !== 'rust') return
  const key = document.uri.toString()
  const timer = pending.get(key)
  if (timer) clearTimeout(timer)
  pending.set(
    key,
    setTimeout(() => {
      pending.delete(key)
      refreshDocument(document)
    }, 150),
  )
}

function refreshEditors(): void {
  for (const editor of vscode.window.visibleTextEditors) {
    if (editor.document.languageId === 'rust') refreshDocument(editor.document)
    else decoration && editor.setDecorations(decoration, [])
  }
}

function refreshAllDocuments(): void {
  for (const document of vscode.workspace.textDocuments) {
    if (document.languageId === 'rust') refreshDocument(document)
  }
  refreshEditors()
}

function refreshDocument(document: vscode.TextDocument): void {
  if (document.isClosed) return
  const usages = analyze(document)
  updateDiagnostics(document, usages)
  for (const editor of vscode.window.visibleTextEditors) {
    if (editor.document.uri.toString() === document.uri.toString()) decorate(editor, usages)
  }
}

function analyze(document: vscode.TextDocument): Usage[] {
  return findUsages(document.getText(), offset => document.positionAt(offset))
}

function updateDiagnostics(document: vscode.TextDocument, usages: Usage[]): void {
  if (!settings.diagnostics) {
    diagnostics.delete(document.uri)
    return
  }
  const locales = index.localeCodes
  const reported = new Set<string>()
  const result: vscode.Diagnostic[] = []
  for (const usage of usages) {
    if (reported.has(usage.key) || isIgnored(usage.key)) continue
    const hit = index.lookup(usage.key)
    if (!hit) {
      result.push(createWarning(usage.range, `未在 locales 中找到键 "${usage.key}"`))
      reported.add(usage.key)
      continue
    }
    const missing = locales.filter(locale => !hit.has(locale))
    if (!missing.length) continue
    result.push(createWarning(usage.range, `键 "${usage.key}" 缺少 ${missing.join('、')} 译文`))
    reported.add(usage.key)
  }
  diagnostics.set(document.uri, result)
}

function createWarning(range: vscode.Range, message: string): vscode.Diagnostic {
  const diagnostic = new vscode.Diagnostic(range, message, vscode.DiagnosticSeverity.Warning)
  diagnostic.source = 'i18n Lens'
  return diagnostic
}

function decorate(editor: vscode.TextEditor, usages: Usage[]): void {
  if (!decoration) return
  if (!settings.annotate) {
    editor.setDecorations(decoration, [])
    return
  }
  const locale = displayLocale()
  const options: vscode.DecorationOptions[] = []
  for (const usage of usages) {
    if (isIgnored(usage.key)) continue
    const hit = index.lookup(usage.key)
    const translation = hit?.get(locale)
    if (translation) {
      options.push({
        range: usage.range,
        renderOptions: { after: { contentText: ` ${shorten(translation.text)}` } },
      })
      continue
    }
    if (!settings.annotateMissing) continue
    const hint = hit ? `${locale} 缺译` : '未定义'
    options.push({ range: usage.range, renderOptions: { after: { contentText: ` ${hint}` } } })
  }
  editor.setDecorations(decoration, options)
}

function provideHover(document: vscode.TextDocument, position: vscode.Position): vscode.Hover | undefined {
  const usage = usageAt(document, position)
  if (!usage) return undefined

  const markdown = new vscode.MarkdownString()
  markdown.appendMarkdown(`**\`${usage.key}\`**\n\n`)

  const hit = index.lookup(usage.key)
  if (!hit) {
    markdown.appendMarkdown('未在 locales 中找到该键')
    return new vscode.Hover(markdown, usage.range)
  }

  const locales = [...hit.keys()].sort(compareLocale)
  markdown.appendMarkdown('| 语言 | 译文 |\n| --- | --- |\n')
  for (const locale of locales) {
    const translation = hit.get(locale)
    if (!translation) continue
    markdown.appendMarkdown(`| ${locale} | ${escapeMarkdown(translation.text)} |\n`)
  }

  const missing = index.localeCodes.filter(locale => !hit.has(locale))
  if (missing.length) markdown.appendMarkdown(`\n缺少：${missing.join('、')}`)
  return new vscode.Hover(markdown, usage.range)
}

function provideDefinition(
  document: vscode.TextDocument,
  position: vscode.Position,
): vscode.Location[] | undefined {
  const usage = usageAt(document, position)
  if (!usage) return undefined
  const entries = index.all(usage.key)
  if (!entries.length) return undefined
  return entries.map(entry => new vscode.Location(entry.uri, entry.range))
}

function usageAt(document: vscode.TextDocument, position: vscode.Position): Usage | undefined {
  for (const usage of analyze(document)) {
    if (usage.range.contains(position)) return usage
  }
  return undefined
}

function displayLocale(): string {
  const locales = index.localeCodes
  const wanted = (settings.displayLanguage || vscode.env.language || settings.sourceLanguage).toLowerCase()
  const exact = locales.find(locale => locale.toLowerCase() === wanted)
  if (exact) return exact
  const base = wanted.split(/[-_]/)[0]
  const partial = locales.find(locale => locale.toLowerCase().split(/[-_]/)[0] === base)
  return partial ?? settings.sourceLanguage
}

function compareLocale(a: string, b: string): number {
  if (a === settings.sourceLanguage) return -1
  if (b === settings.sourceLanguage) return 1
  return a.localeCompare(b)
}

function isIgnored(key: string): boolean {
  return settings.ignoreKeyPrefixes.some(prefix => prefix.length > 0 && key.startsWith(prefix))
}

function isLocaleFile(uri: vscode.Uri): boolean {
  if (uri.scheme !== 'file') return false
  const relative = vscode.workspace.asRelativePath(uri, false).replace(/\\/g, '/')
  if (!/\.(ya?ml|json)$/i.test(relative)) return false
  return settings.localesPaths.some(localePath => {
    const dir = localePath.replace(/^\.?\//, '').replace(/\/+$/, '')
    return dir.length > 0 && relative.startsWith(`${dir}/`)
  })
}

function shorten(text: string): string {
  const single = text.replace(/\s+/g, ' ').trim()
  return single.length > 90 ? `${single.slice(0, 90)}…` : single
}

function escapeMarkdown(text: string): string {
  return text.replace(/\|/g, '\\|').replace(/\r?\n/g, ' ')
}

function readSettings(): Settings {
  const config = vscode.workspace.getConfiguration('i18nLens')
  return {
    localesPaths: config.get<string[]>('localesPaths', ['locales']),
    sourceLanguage: config.get<string>('sourceLanguage', 'en'),
    displayLanguage: config.get<string>('displayLanguage', ''),
    annotate: config.get<boolean>('annotation.enabled', true),
    annotateMissing: config.get<boolean>('annotation.missing', true),
    diagnostics: config.get<boolean>('diagnostics.enabled', true),
    ignoreKeyPrefixes: config.get<string[]>('ignoreKeyPrefixes', []),
  }
}
