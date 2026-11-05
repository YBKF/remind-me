// "version"、"platforms.[target].url" 和 "platforms.[target].signature" 是必需的；其它字段是可选的。
// deno-lint-ignore-file no-explicit-any
import { parseArgs } from "jsr:@std/cli@1.0.32";

const GITEE_REP_RELEASE_LATEST =
    "https://gitee.com/YBKF/remind-me/releases/latest";

function tryUpdateMap(
    key: string,
    value: any | undefined | null,
    map: Map<string, any>,
    required: boolean = true,
) {
    if (null == value) {
        if (required) {
            const errorInfo = `(Required) Invalid "${key}" field: ${
                value ? JSON.stringify(value) : "null"
            }`;
            console.error(errorInfo);
            throw new Error(errorInfo);
        }

        // 跳过，不执行操作
        return;
    }

    map.set(key, value);
}

interface AttachFile {
    download_url: string;
    name: string;
    cli_download_url: string;
    censor_failed: boolean;
    censor_tip?: string;
}

try {
    const args = parseArgs(Deno.args, {
        string: ["sig-path", "output"],
    });

    console.log(args);

    const response = await fetch(
        GITEE_REP_RELEASE_LATEST,
    );

    if (!response.ok) {
        console.error("指定仓库不存在最新发行页");
        throw new Error("指定仓库不存在最新发行页");
    }

    const response_json = await response.json();
    const attach_files: AttachFile[] =
        response_json.release.release.attach_files;
    const exe_attach_file = attach_files.filter((item) =>
        item.name.includes(".exe")
    )[0];

    if (null == exe_attach_file) {
        console.error("最新发行版中不存在 exe 文件");
        throw new Error("最新发行版中不存在 exe 文件");
    }

    const sig_path = args["sig-path"];
    if (!sig_path) {
        console.error("未指定签名文件路径");
        throw new Error("未指定签名文件路径");
    }

    const output_path = args["output"];
    if (!output_path) {
        console.error("未指定 version.json 的输出路径");
        throw new Error("未指定 version.json 的输出路径");
    }

    const signature = Deno.readTextFileSync(sig_path);

    const version_map = new Map<string, any>();

    tryUpdateMap("version", response_json.release.tag.name, version_map);
    tryUpdateMap(
        "notes",
        response_json.release.tag.message,
        version_map,
        false,
    );
    tryUpdateMap(
        "pub_date",
        response_json.release.release.created_at,
        version_map,
        false,
    );
    tryUpdateMap(
        "platforms",
        {
            "windows-x86_64": {
                "signature": signature,
                "url": exe_attach_file.cli_download_url,
            },
        },
        version_map,
    );

    Deno.writeTextFileSync(
        output_path,
        JSON.stringify(Object.fromEntries(version_map)),
    );
    Deno.exit(0);
} catch (e) {
    console.error(e);
    Deno.exit(1);
}
