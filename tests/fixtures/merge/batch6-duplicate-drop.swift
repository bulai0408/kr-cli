    @Test func v14UpgradePreservesWatchDataAndStartsWithDefaultGroups() async throws {
        let url = URL.temporaryDirectory.appending(path: "groups-v14-\(UUID()).sqlite")
        let previous = try UserStateStore(databaseURL: url)
        let title = TitleID(kind: .movie, tmdbID: 42)
        try await previous.setWatchlisted(true, title: title)
        let fixture = try DatabaseQueue(path: url.path)
        try await fixture.write { db in
            try db.execute(sql: "DROP TABLE moduleConfigurations")
            try db.execute(sql: "DROP TABLE sourceGroups")
            try db.execute(sql: "DROP TABLE favorites")
            try db.execute(sql: "DROP TABLE watchlistSeriesEpisodes")
            try db.execute(sql: "DROP TABLE moduleWatchlist")
            try db.execute(sql: "DROP TABLE moduleConfigurations")
            try db.execute(sql: "DELETE FROM grdb_migrations WHERE identifier IN ('UserState.v15', 'UserState.v16', 'UserState.v17', 'UserState.v18')")
            try db.execute(sql: "PRAGMA user_version = 14")
        }
        let upgraded = try UserStateStore(databaseURL: url)
        #expect(try await upgraded.watchlist(for: title).isListed)
        #expect(try await upgraded.sourceGroups() == SourceGroupsConfiguration())
        try await upgraded.database.read { db in
            let version = try Int.fetchOne(db, sql: "PRAGMA user_version")
            #expect(version == 18)
        }
    }
