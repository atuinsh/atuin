import Database from "@tauri-apps/plugin-sql";
import { uuidv7 } from "uuidv7";

export default class Runbook {
  id: string;
  created: Date;
  updated: Date;

  private _name: string;
  private _content: string;

  set name(value: string) {
    this.updated = new Date();
    this._name = value;
  }

  set content(value: string) {
    this.updated = new Date();
    this._content = value;
  }

  get content() {
    return this._content;
  }

  get name() {
    return this._name;
  }

  constructor(
    id: string,
    name: string,
    content: string,
    created: Date,
    updated: Date,
  ) {
    this.id = id;
    this._name = name;
    this._content = content;
    this.created = created;
    this.updated = updated;
  }

  /// Create a new Runbook, and automatically generate an ID.
  public static async create(): Promise<Runbook> {
    let now = new Date();

    // Initialize with the same value for created/updated, to avoid needing null.
    let runbook = new Runbook(uuidv7(), "", "", now, now);
    await runbook.save();

    return runbook;
  }

  public static async load(id: String): Promise<Runbook | null> {
    const db = await Database.load("sqlite:runbooks.db");

    let res = await db.select<any[]>("select * from runbooks where id = $1", [
      id,
    ]);

    if (res.length == 0) return null;

    let rb = res[0];

    return new Runbook(
      rb.id,
      rb.name,
      rb.content,
      new Date(rb.created / 1000000),
      new Date(rb.updated / 1000000),
    );
  }

  static async all(): Promise<Runbook[]> {
    const db = await Database.load("sqlite:runbooks.db");

    let res = await db.select<any[]>(
      "select * from runbooks order by updated desc",
    );

    return res.map((i) => {
      return new Runbook(
        i.id,
        i.name,
        i.content,
        new Date(i.created / 1000000),
        new Date(i.updated / 1000000),
      );
    });
  }

  public async save() {
    const db = await Database.load("sqlite:runbooks.db");

    await db.execute(
      `insert into runbooks(id, name, content, created, updated)
          values ($1, $2, $3, $4, $5)

          on conflict(id) do update
            set
              name=$2,
              content=$3,
              updated=$5`,

      // getTime returns a timestamp as unix milliseconds
      // we won't need or use the resolution here, but elsewhere Atuin stores timestamps in sqlite as nanoseconds since epoch
      // let's do that across the board to avoid mistakes
      [
        this.id,
        this._name,
        this._content,
        this.created.getTime() * 1000000,
        this.updated.getTime() * 1000000,
      ],
    );
  }

  public static async delete(id: string) {
    const db = await Database.load("sqlite:runbooks.db");

    await db.execute("delete from runbooks where id=$1", [id]);
  }
}
