mod phab;
mod arcconfig;
mod check;
mod fmt;

/// Context containing data typically shared between the subcommands.
struct Context {
    phab_uri: String,
    build_phid: String,
    token: String,
    arcconfig: std::path::PathBuf,
}

fn subcommand_args<'a, 'b>(sc: clap::App<'a, 'b>) -> clap::App<'a, 'b> {
    sc.arg(clap::Arg::with_name("args").raw(true))
}

#[derive(thiserror::Error, Debug)]
#[error("could not find the .arcconfig")]
struct FindArcConfigError(#[source] crate::arcconfig::Error);

#[derive(thiserror::Error, Debug)]
#[error("phabricator.uri not specified in .arcconfig nor is --phabricator-uri")]
struct GetLocationError;

#[derive(thiserror::Error, Debug)]
#[error("--build-phid not available")]
struct GetBuildPhidError;

#[derive(thiserror::Error, Debug)]
#[error("--conduit-token not available")]
struct GetConduitTokenError;



fn main() {
    let fmt_subcommand = subcommand_args(clap::SubCommand::with_name("fmt"));
    let check_subcommand = subcommand_args(clap::SubCommand::with_name("check"));
    let build_subcommand = subcommand_args(clap::SubCommand::with_name("build"));
    let test_subcommand = subcommand_args(clap::SubCommand::with_name("test"));

    let cli = clap::App::new(clap::crate_name!())
        .version(clap::crate_version!())
        .author(clap::crate_authors!())
        .about(clap::crate_description!())
        .arg(
            // Cargo passes in the subcommand passed to it as the first argument to the executable
            // it invokes. This hidden argument handles it.
            clap::Arg::with_name("dummy")
                .possible_value("phabricator")
                .required(false)
                .hidden(true)
        )
        .arg(
            clap::Arg::with_name("phabricator_uri")
                .long("phabricator-uri")
                .help("Address at which to find Phabricator. \
                    `  .arcconfig` may be used for defaults")
                .takes_value(true)
                .required(false)
                .env("PHABRICATOR_URI")
        )
        .arg(
            clap::Arg::with_name("conduit_token")
                .long("conduit-token")
                .help("API token to use when contacting Phabricator")
                .required(true)
                .takes_value(true)
                .env("CONDUIT_TOKEN")
        )
        .arg(
            clap::Arg::with_name("build_phid")
                .long("build-phid")
                .help("The PHID of the Harbormaster build that should receive results")
                .required(true)
                .takes_value(true)
                .env("BUILD_PHID")
        )
        .subcommand(fmt_subcommand)
        .subcommand(check_subcommand)
        .subcommand(build_subcommand)
        .subcommand(test_subcommand);

    let matches = cli.get_matches();
    let result: Result<(), Box<dyn std::error::Error>> = tokio::runtime::Builder::new()
        .basic_scheduler()
        .enable_all()
        .build()
        .map_err(Into::into)
        .and_then(|mut runtime| runtime.block_on(async {
            let arcconfig = crate::arcconfig::find().map_err(FindArcConfigError)?;
            let phab_uri = matches.value_of("phabricator_uri")
                .or(arcconfig.phab_uri.as_ref().map(|x| &x[..]))
                .ok_or(GetLocationError)?;
            let build_phid = matches.value_of("build_phid")
                .ok_or(GetBuildPhidError)?;
            let token = matches.value_of("conduit_token")
                .ok_or(GetConduitTokenError)?;

            let ctxt = Context {
                phab_uri: String::from(phab_uri),
                build_phid: String::from(build_phid),
                token: String::from(token),
                arcconfig: arcconfig.location,
            };
            match matches.subcommand() {
                ("fmt", Some(args)) => ctxt.fmt(args).await.map_err(Into::into),
                ("check", Some(args)) => check::run(args).await,
                ("build", Some(args)) => Ok(()),
                ("test", Some(args)) => Ok(()),
                (sc, Some(args)) => Err("unimplemented subcommand".into()),
                (sc, None) => Err(format!("clap did not produce args for {}", sc).into()),
            }
        }));

    std::process::exit(match result {
        Ok(_) => 0,
        Err(ref error) => {
            eprintln!("error: {}", error);
            let mut source = error.source();
            while let Some(src) = source {
                eprintln!("  caused by: {}", src);
                source = src.source();
            }
            1
        }
    });
}
