import Database from "@tauri-apps/plugin-sql";
import { uuidv7 } from "uuidv7";

export default class Runbook {
  id: String;
  created: Date;
  updated: Date;

  private _name: String;
  private _content: String;

  set name(value: String) {
    this.updated = new Date();
    this._name = value;
  }

  set content(value: String) {
    this.updated = new Date();
    this._content = value;
  }

  constructor(
    id: String,
    name: String,
    content: String,
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
  static create(name: String, content: String): Runbook {
    let now = new Date();

    // Initialize with the same value for created/updated, to avoid needing null.
    return new Runbook(uuidv7(), name, content, now, now);
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

  async save() {
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
}
